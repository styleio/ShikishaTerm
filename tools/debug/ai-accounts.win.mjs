/**
 * More than one account of an AI's CLI: a tab started as an account is
 * pointed at that account's folder, and nothing else the app does with the
 * CLI's folder breaks.
 *
 * This checkout's build in a folder of its own, with a home folder of its
 * own (so "the usual folder" is a scratch `.claude`, never this PC's). The
 * "AI" is a stand-in under Claude Code's name: it shows the folder it was
 * pointed at, and writes a conversation record where Claude Code would. The
 * settings hold one account, "Work", and two tabs: one running as it, one as
 * the CLI is signed in. Checked:
 *   - the account's tab is pointed at the account's folder, the other at none
 *   - the account's folder is made up: conversations and skills linked to the
 *     usual folder, settings copied, the sign-in not copied, the mark written
 *   - a conversation written through the account lands in the usual folder
 *   - this app's hooks are in the account's settings
 *   - the settings say who the account is signed in as, once the CLI wrote it
 *   - "Sign in", through the phone's door to the settings, opens a tab as the
 *     account
 *   - an account taken out of the settings holds its tab, saying why, after
 *     the app starts again
 *   - deleting the account's folder leaves the usual folder's conversations
 *   - the settings page draws the card, and throws nothing
 *
 *     cargo build
 *     node tools/debug/ai-accounts.win.mjs
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
const RUN = path.join(os.tmpdir(), 'sk-ai-accounts');
const APP = path.join(RUN, 'app');
const HOME = path.join(RUN, 'home');
const WORK = path.join(RUN, 'work');
const PORT = 9481;
const TOKEN = 'accounts-token-0123456789';
const BASE = `http://127.0.0.1:${PORT}`;
const USUAL = path.join(HOME, '.claude');
const ACCOUNT = path.join(APP, 'data', 'ai-accounts', 'work', 'home');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'msedgewebview2.exe' -or $_.Name -eq 'node.exe') -and ($_.CommandLine + '') -like '*sk-ai-accounts*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [path.join(APP, 'config'), WORK, path.join(USUAL, 'projects', 'C--old'), path.join(USUAL, 'skills', 'mine')]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);

// The usual folder as somebody who uses Claude Code has it
fs.writeFileSync(path.join(USUAL, 'settings.json'), JSON.stringify({ model: 'usual-model' }, null, 2));
fs.writeFileSync(path.join(USUAL, '.credentials.json'), '{"claudeAiOauth":{"accessToken":"usual-secret"}}');
fs.writeFileSync(path.join(USUAL, 'projects', 'C--old', 'old-conversation.jsonl'), '{}\n');
fs.writeFileSync(path.join(HOME, '.claude.json'), JSON.stringify({ oauthAccount: { emailAddress: 'usual@example.invalid' }, hasCompletedOnboarding: true, projects: {} }));

// The stand-in: says the folder it was pointed at, and writes a record there
const bin = path.join(RUN, 'bin');
fs.mkdirSync(bin, { recursive: true });
fs.writeFileSync(path.join(bin, 'stand-in.mjs'), `
const fs = require('node:fs'), path = require('node:path');
const dir = process.env.CLAUDE_CONFIG_DIR || '';
process.stdout.write('CFG=[' + dir + ']\\r\\n');
// Said into a file named after the tab too, which is what the checks read
fs.mkdirSync(${JSON.stringify(path.join(RUN, 'said'))}, { recursive: true });
fs.writeFileSync(path.join(${JSON.stringify(path.join(RUN, 'said'))}, (process.env.SHIKISHA_TAB || 'unknown') + '.txt'), 'CFG=[' + dir + ']');
if (dir) {
  const rec = path.join(dir, 'projects', 'C--work');
  fs.mkdirSync(rec, { recursive: true });
  fs.writeFileSync(path.join(rec, 'from-the-account.jsonl'), '{}\\n');
}
process.stdout.write('> ');
setInterval(() => {}, 1000);
`.replace(/require\('node:(fs|path)'\)/g, "require('$1')"));
fs.writeFileSync(path.join(bin, 'stand-in.cjs'), fs.readFileSync(path.join(bin, 'stand-in.mjs')));
fs.writeFileSync(path.join(bin, 'claude.cmd'), `@echo off\r\nnode "${path.join(bin, 'stand-in.cjs')}"\r\n`);

const config = (accounts) => ({
  language: 'en', agent_hooks: { 'Claude Code': 'on' },
  // On, so each tab is told who it is (SHIKISHA_TAB), which the stand-in
  // names what it says after
  external_api: { access: 'user' },
  ai_accounts: accounts,
  remote: { enabled: true, bind: '127.0.0.1', port: PORT, sticky_token: true, fixed_token: TOKEN },
  desks: [{ name: 'Two', id: 'two', folders: [{ cwd: WORK, tabs: [
    { name: 'worker', id: 'worker', command: ['claude'], ai_account: 'work' },
    { name: 'plain', id: 'plain', command: ['claude'] },
  ] }] }],
});
const configFile = path.join(APP, 'config', 'config.json');
fs.writeFileSync(configFile, JSON.stringify(config([{ id: 'work', cli: 'claude', name: 'Work' }]), null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|CODEX)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.USERPROFILE = HOME;
env.HOME = HOME;
env.PATH = bin + ';' + (env.PATH || env.Path || '');
delete env.Path;
const start = () => spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

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
  while (Date.now() < end) { try { const v = await test(); if (v) return v; } catch {} await sleep(800); }
  throw new Error('timed out waiting for ' + what);
};
const state = async () => { const s = await (await send(`/api/state?t=${TOKEN}`)).json(); return s.ui || s; };
const tabs = async () => (await state()).tabs || [];
// What the stand-in in a tab said, by the tab's automation name
const screenOf = async (id) => {
  const f = path.join(RUN, 'said', id + '.txt');
  return fs.existsSync(f) ? fs.readFileSync(f, 'utf8') : '';
};
const linked = (p) => { try { return fs.lstatSync(p).isSymbolicLink() || fs.realpathSync(p) !== p; } catch { return false; } };

let chrome;
try {
  start();
  await until(async () => (await send(`/?t=${TOKEN}`)).status < 500 && jar.has('rs'), 'the board', 60000);
  await until(async () => (await tabs()).length >= 2, 'the tabs', 30000);

  // Pointed at the account's folder, and the other at none
  const workerSays = await until(async () => { const s = await screenOf('worker'); return /CFG=\[/.test(s) && s; }, 'the worker to say its folder', 30000).catch(() => '');
  const plainSays = await until(async () => { const s = await screenOf('plain'); return /CFG=\[/.test(s) && s; }, 'the plain tab to say its folder', 30000).catch(() => '');
  const shown = (workerSays.match(/CFG=\[([^\]]*)\]/) || [])[1] || '';
  check(shown && path.resolve(shown).toLowerCase() === ACCOUNT.toLowerCase(), `the account's tab is pointed at the account's folder (${shown})`);
  check(/CFG=\[\]/.test(plainSays), `the other tab is pointed at no folder (${(plainSays.match(/CFG=\[[^\]]*\]/) || [''])[0]})`);

  // Made up from the usual folder
  check(linked(path.join(ACCOUNT, 'projects')) && fs.existsSync(path.join(ACCOUNT, 'projects', 'C--old', 'old-conversation.jsonl')), 'the account sees the usual folder\'s conversations through a link');
  check(linked(path.join(ACCOUNT, 'skills')) && fs.existsSync(path.join(ACCOUNT, 'skills', 'mine')), 'the account sees the usual skills');
  check(JSON.parse(fs.readFileSync(path.join(ACCOUNT, 'settings.json'), 'utf8')).model === 'usual-model', 'the settings start as the usual ones');
  check(!fs.existsSync(path.join(ACCOUNT, '.credentials.json')), 'the usual sign-in is not copied into the account');
  const state0 = fs.existsSync(path.join(ACCOUNT, '.claude.json')) ? JSON.parse(fs.readFileSync(path.join(ACCOUNT, '.claude.json'), 'utf8')) : {};
  check(state0.hasCompletedOnboarding === true && !state0.oauthAccount, 'the state file comes without the account it was read from');
  check(fs.existsSync(path.join(APP, 'data', 'ai-accounts', 'work', 'account.json')), 'the folder carries the mark of an account\'s');
  check(fs.existsSync(path.join(USUAL, 'projects', 'C--work', 'from-the-account.jsonl')), 'a conversation written through the account is in the usual folder');

  // The app's hooks are in the account's settings
  const hooked = await until(() => { const t = fs.readFileSync(path.join(ACCOUNT, 'settings.json'), 'utf8'); return /SessionStart/.test(t) && t.includes('SHIKISHA-TERM') && t; }, 'the hooks in the account', 20000).catch(() => '');
  check(!!hooked, 'this app\'s hooks are in the account\'s settings');

  // Who it is signed in as, as the CLI wrote it
  await send(`/cfg?t=${TOKEN}`);
  const before = await until(async () => { const r = await send('/api/ai-accounts'); return r.status === 200 && (await r.json()); }, 'the settings', 30000);
  check(before.accounts.length === 1 && before.accounts[0].who === null && before.accounts[0].made, `an account not signed in says so (${JSON.stringify(before.accounts[0])})`);
  const st = JSON.parse(fs.readFileSync(path.join(ACCOUNT, '.claude.json'), 'utf8'));
  st.oauthAccount = { emailAddress: 'work@example.invalid' };
  fs.writeFileSync(path.join(ACCOUNT, '.claude.json'), JSON.stringify(st));
  const after = await (await send('/api/ai-accounts')).json();
  check(after.accounts[0].who === 'work@example.invalid', `the settings say who it is signed in as (${after.accounts[0].who})`);

  // Sign in: a tab opened as the account
  const asked = await post('/api/ai-accounts/login', { id: 'work' });
  check(asked.ok === true, `"Sign in" is taken (${JSON.stringify(asked)})`);
  const opened = await until(async () => (await tabs()).find((t) => /Work/.test(t.name)), 'the sign-in tab', 30000).catch(() => null);
  check(!!opened, `a tab opens as the account (${opened && opened.name})`);
  const openedSays = opened ? await until(async () => { const s = await screenOf(opened.id || opened.name); return /CFG=\[/.test(s) && s; }, 'the sign-in tab to start', 30000).catch(() => '') : '';
  check(openedSays.toLowerCase().includes(ACCOUNT.toLowerCase()), 'the sign-in tab runs as the account');
  const written = JSON.parse(fs.readFileSync(configFile, 'utf8'));
  check(JSON.stringify(written.desks).includes('"ai_account": "work"') || JSON.stringify(written.desks).includes('"ai_account":"work"'), 'the sign-in tab is written as running as the account');

  // The page draws the card
  chrome = await startChrome();
  await chrome.send('Emulation.setDeviceMetricsOverride', { width: 420, height: 900, deviceScaleFactor: 2, mobile: true });
  await chrome.send('Page.navigate', { url: `${BASE}/?t=${TOKEN}` });
  await until(() => chrome.run(`typeof S !== "undefined" && !!S`), 'the board in the browser', 30000);
  await chrome.send('Page.navigate', { url: `${BASE}/cfg?section=aiaccounts` });
  const row = await until(() => chrome.run(`(() => { const r = document.querySelector("[data-account=work]"); return r ? r.textContent : ""; })()`), 'the card', 30000).catch(() => '');
  check(/Work/.test(row) && /work@example\.invalid/.test(row), `the card lists the account and who it is (${row.slice(0, 120)})`);
  check(chrome.thrown.length === 0, `the page threw nothing (${chrome.thrown.join(' | ')})`);
  chrome.stop();
  chrome = null;

  // Taken out of the settings: its tab holds, after a start
  const gone = JSON.parse(fs.readFileSync(configFile, 'utf8'));
  gone.ai_accounts = [];
  fs.writeFileSync(configFile, JSON.stringify(gone, null, 2));
  stopApp();
  await sleep(1500);
  start();
  await until(async () => (await send(`/?t=${TOKEN}`)).status < 500, 'the board again', 60000);
  const held = await until(async () => (await tabs()).find((t) => t.name === 'worker' && t.hold), 'the worker to be held', 30000).catch(() => null);
  check(held && held.hold.kind === 'account', `a tab of an account no longer in the settings is held, and says why (${held && JSON.stringify(held.hold).slice(0, 160)})`);

  // Deleting the folder leaves the usual folder's conversations
  await send(`/cfg?t=${TOKEN}`);
  const dropped = await until(async () => { const r = await send('/api/ai-accounts/forget', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ id: 'work' }) }); return r.status === 200 && (await r.json()); }, 'the delete', 20000);
  check(dropped.ok === true && !fs.existsSync(path.join(APP, 'data', 'ai-accounts', 'work')), `the account's folder is deleted (${JSON.stringify(dropped)})`);
  check(fs.existsSync(path.join(USUAL, 'projects', 'C--old', 'old-conversation.jsonl')) && fs.existsSync(path.join(USUAL, 'projects', 'C--work', 'from-the-account.jsonl')), 'the usual folder\'s conversations are all still there');
  check(fs.existsSync(path.join(USUAL, '.credentials.json')), 'the usual sign-in is still there');
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  if (chrome) chrome.stop();
  if (!process.env.KEEP) stopApp();
}
console.log(failures ? `${failures} check(s) failed` : 'all checks passed');
process.exit(failures ? 1 : 0);
