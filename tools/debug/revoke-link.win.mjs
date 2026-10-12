/**
 * Taking a device's key away also stops the link it came in by.
 *
 * This checkout's build in a folder of its own, with its board on this
 * machine only. Two devices open the link and are written into the book; the
 * second takes the first one's key away from the settings, the way the
 * settings screen does. Checked:
 *   - the board's link is changed, and the QR shows the new one
 *   - the removed device, opening the old link again, is not let back in
 *     and is not written into the book as a new device
 *   - the other device goes on reading the board with its own key
 *   - a new device comes in by the new link
 *
 *     cargo build
 *     node tools/debug/revoke-link.win.mjs
 *
 * Needs Windows and Node. Nothing of a copy somebody is using is read,
 * written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-revoke-link');
const APP = path.join(RUN, 'app');
const PORT = 9468;
const BASE = `http://127.0.0.1:${PORT}`;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and ($_.CommandLine + '') -like '*sk-revoke-link*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
fs.mkdirSync(path.join(APP, 'config'), { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);
const answered = {};
for (const f of fs.readdirSync(path.join(ROOT, 'profiles')).filter((f) => f.endsWith('.json'))) {
  const p = JSON.parse(fs.readFileSync(path.join(ROOT, 'profiles', f), 'utf8'));
  if (p.resume && p.resume.hook) answered[p.name] = 'off';
}
fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
  language: 'en',
  agent_hooks: answered,
  remote: { enabled: true, bind: '127.0.0.1', port: PORT },
  desks: [{ name: 'Revoke', id: 'revoke', folders: [{ cwd: RUN, tabs: [{ name: 'shell', id: 'shell', command: ['cmd.exe'] }] }] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

// A browser: the cookies it is handed, kept and sent back
const device = () => {
  const jar = new Map();
  const send = async (url, init = {}) => {
    const r = await fetch(BASE + url, { redirect: 'manual', ...init,
      headers: { ...(init.headers || {}), Cookie: [...jar].map(([k, v]) => `${k}=${v}`).join('; ') },
      signal: AbortSignal.timeout(15000) });
    const set = r.headers.getSetCookie().map((c) => c.split(';')[0]);
    for (const kv of set) { const i = kv.indexOf('='); jar.set(kv.slice(0, i), kv.slice(i + 1)); }
    return { r, set };
  };
  return { send, jar };
};
const tokenFile = path.join(APP, 'data', 'remote-token');
const token = () => (fs.existsSync(tokenFile) ? fs.readFileSync(tokenFile, 'utf8').trim() : '');
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return; await sleep(500); }
  throw new Error('timed out waiting for ' + what);
};

try {
  await until(async () => token() && (await fetch(BASE + '/manifest.webmanifest')).status < 500, 'the board', 60000);
  const first = token();
  const phone = device(), laptop = device();
  await phone.send(`/?t=${first}`);
  await laptop.send(`/?t=${first}`);
  check(phone.jar.has('rk') && laptop.jar.has('rk'), 'both devices are handed keys of their own');

  // The laptop opens the settings through the board and takes the phone's key
  await laptop.send(`/cfg?t=${first}`);
  await until(async () => (await laptop.send('/api/remote/clients')).r.status === 200, 'the settings', 30000);
  const list = async () => (await (await laptop.send('/api/remote/clients')).r.json()).clients;
  const rows = await list();
  check(rows.length === 2, `two devices in the book (${rows.length})`);
  // The phone is the one whose session the board ends: the first row written
  const phoneRow = rows.sort((a, b) => a.added - b.added)[0];
  const qrBefore = await (await laptop.send('/api/remote/qr')).r.text();
  const revoked = await (await laptop.send('/api/remote/clients/revoke', {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ id: phoneRow.id }),
  })).r.json();
  check(revoked.ok === true, 'the key is taken away');

  await until(async () => token() !== first, 'the link to change', 10000).catch((e) => check(false, e.message));
  check(token() !== first, 'the board\'s link is changed');
  const qrAfter = await (await laptop.send('/api/remote/qr')).r.text();
  check(qrAfter && qrAfter !== qrBefore, 'the QR shows the new link');

  const back = device();
  for (const [k, v] of phone.jar) back.jar.set(k, v);
  const { set } = await back.send(`/?t=${first}`);
  check(!set.some((c) => c.startsWith('rs=') || c.startsWith('rk=')), `the removed device is not let back in by the old link (${set.join(', ') || 'no cookies'})`);
  check((await back.send(`/api/state?t=${first}`)).r.status === 403, 'the removed device cannot read the board');
  check((await list()).length === 1, 'the old link wrote no new device into the book');

  check((await laptop.send(`/api/state?t=${first}`)).r.status === 200, 'the other device goes on reading the board with its own key');

  const newcomer = device();
  await newcomer.send(`/?t=${token()}`);
  check((await newcomer.send('/api/state')).r.status === 200, 'a new device comes in by the new link');
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  stopApp();
}
console.log(failures ? `${failures} check(s) failed` : 'all checks passed');
process.exit(failures ? 1 : 0);
