/**
 * What a tab said while another was being looked at is marked unread, and
 * the mark goes once it is looked at.
 *
 * This checkout's build in a folder of its own, with two AI tabs -- stand-ins
 * named `claude` first on the PATH that, given a line, show Claude Code's
 * "esc to interrupt" for a few seconds and then answer, so no AI runs -- and
 * its board on this machine only. Checked:
 *   - a tab that finishes while another is in front is unread, and the one
 *     in front is not
 *   - a tab that finishes while it is in front is not unread
 *   - bringing the unread tab to the front takes its mark away
 *
 *     cargo build
 *     node tools/debug/unread-mark.win.mjs
 *
 * Needs Windows and Node. Nothing of a copy somebody is using is read,
 * written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-unread-mark');
const APP = path.join(RUN, 'app');
const PORT = 9476;
const TOKEN = 'unread-mark-token-0123456789';
const BASE = `http://127.0.0.1:${PORT}`;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'msedgewebview2.exe' -or $_.Name -eq 'node.exe') -and ($_.CommandLine + '') -like '*sk-unread-mark*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
fs.mkdirSync(path.join(APP, 'config'), { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);

// The stand-in: a prompt, and for each line typed a few seconds of Claude
// Code's working line, then an answer and the prompt again
const bin = path.join(RUN, 'bin');
fs.mkdirSync(bin, { recursive: true });
fs.writeFileSync(path.join(bin, 'stand-in.mjs'), `
const out = (s) => process.stdout.write(s);
out('> ');
process.stdin.setRawMode && process.stdin.setRawMode(true);
let line = '';
process.stdin.on('data', (d) => {
  for (const c of d.toString()) {
    if (c === '\\r' || c === '\\n') {
      const said = line; line = '';
      out('\\r\\n* Working (esc to interrupt)');
      setTimeout(() => { out('\\r\\x1b[2K' + 'answer to ' + said + '\\r\\n> '); }, 4000);
    } else { line += c; out(c); }
  }
});
`);
fs.writeFileSync(path.join(bin, 'claude.cmd'), `@echo off\r\nnode "${path.join(bin, 'stand-in.mjs')}"\r\n`);

const answered = {};
for (const f of fs.readdirSync(path.join(ROOT, 'profiles')).filter((f) => f.endsWith('.json'))) {
  const p = JSON.parse(fs.readFileSync(path.join(ROOT, 'profiles', f), 'utf8'));
  if (p.resume && p.resume.hook) answered[p.name] = 'off';
}
fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
  language: 'en', agent_hooks: answered, external_api: { access: 'user' },
  remote: { enabled: true, bind: '127.0.0.1', port: PORT, sticky_token: true, fixed_token: TOKEN },
  desks: [{ name: 'Unread', id: 'unread', folders: [{ cwd: RUN, tabs: [
    { name: 'front', id: 'front', command: ['claude'] },
    { name: 'behind', id: 'behind', command: ['claude'] },
  ] }] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.PATH = bin + ';' + (env.PATH || env.Path || '');
delete env.Path;
const child = spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' });
const pid = child.pid;
child.unref();

// The door, to type into a tab the way a person's keys reach it
let door;
const openDoor = async () => {
  const tokenFile = path.join(APP, 'data', 'api-token');
  const end = Date.now() + 60000;
  while (!fs.existsSync(tokenFile) && Date.now() < end) await sleep(500);
  const sock = net.connect(`\\\\.\\pipe\\shikisha-${pid}`);
  await new Promise((r, j) => { sock.once('connect', r); sock.once('error', j); });
  let buf = '';
  const waiting = [];
  sock.on('data', (d) => {
    buf += d.toString('utf8');
    let i;
    while ((i = buf.indexOf('\n')) >= 0) { const l = buf.slice(0, i); buf = buf.slice(i + 1); const w = waiting.shift(); if (w) w(JSON.parse(l)); }
  });
  const line = (o) => new Promise((r) => { waiting.push(r); sock.write(JSON.stringify(o) + '\n'); });
  await line({ token: fs.readFileSync(tokenFile, 'utf8').trim() });
  let n = 0;
  door = async (method, ...params) => { const a = await line({ id: String(++n), method, params }); if (!a.ok) throw new Error(`${method}: ${a.error}`); return a.result; };
};
let cookies = '';
const state = async () => (await (await fetch(`${BASE}/api/state?t=${TOKEN}`, { headers: { Cookie: cookies } })).json());
const tabs = async () => { const s = await state(); return Object.fromEntries(((s.ui || s).tabs || []).map((t) => [t.name, t])); };
const intent = (o) => fetch(`${BASE}/api/intent?t=${TOKEN}`, { method: 'POST', headers: { Cookie: cookies }, body: JSON.stringify(o) });
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { try { const v = await test(); if (v) return v; } catch {} await sleep(300); }
  throw new Error('timed out waiting for ' + what);
};
const ask = async (tab, words) => { await door('send', tab, words); await sleep(300); await door('send', tab, '\r'); };

try {
  await openDoor();
  await until(async () => { const r = await fetch(`${BASE}/?t=${TOKEN}`, { redirect: 'manual' }); cookies = r.headers.getSetCookie().map((c) => c.split(';')[0]).join('; '); return /rs=/.test(cookies); }, 'the board', 60000);
  const first = await until(async () => { const t = await tabs(); return t.front && t.behind && t; }, 'the tabs', 30000);
  await intent({ kind: 'select', tab: first.front.index });
  await until(async () => /> /.test(String(await door('tab_screen', 'behind'))), 'the stand-ins to come up', 30000);

  // The tab behind answers while the front one is looked at
  await ask('behind', 'one');
  await until(async () => (await tabs()).behind.state === 'BUSY', 'the tab behind to work', 15000);
  await until(async () => (await tabs()).behind.state === 'DONE', 'the tab behind to answer', 30000);
  let t = await tabs();
  check(t.behind.unread === true, 'what the tab behind said is unread');
  check(!t.front.unread, 'the tab in front is not');

  // The front one answers while it is looked at
  await ask('front', 'two');
  await until(async () => (await tabs()).front.state === 'BUSY', 'the front tab to work', 15000);
  await until(async () => (await tabs()).front.state === 'DONE', 'the front tab to answer', 30000);
  check(!(await tabs()).front.unread, 'what the tab in front said is not unread: it was being looked at');

  // Looked at, the mark goes
  await intent({ kind: 'select', tab: t.behind.index });
  await until(async () => !(await tabs()).behind.unread, 'the mark to go', 5000).catch(() => {});
  check(!(await tabs()).behind.unread, 'bringing it to the front takes the mark away');
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  if (!process.env.KEEP) stopApp();
}
console.log(failures ? `${failures} check(s) failed` : 'all checks passed');
process.exit(failures ? 1 : 0);
