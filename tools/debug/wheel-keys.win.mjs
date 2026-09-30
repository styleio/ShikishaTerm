/**
 * Where a turn of the wheel goes in a full-screen program, through the
 * running app.
 *
 *     node tools/debug/wheel-keys.win.mjs
 *
 * Needs Windows and a build (`cargo build --bin SHIKISHA-TERM`). It lays out
 * a copy of the app of its own under D:\ShikishaTerm-wheel (or %TEMP% where
 * there is no D:), runs `alt-scroll-tui.mjs` in three tabs, and turns the
 * wheel through the board's own intent -- the road the window's wheel and the
 * phone's page buttons take. Every process it stops is matched by FULL PATH.
 *
 * Why it exists. Codex opens its transcript on the alternate screen and asks
 * for the wheel as arrow keys (CSI ? 1007 h) without watching the mouse. The
 * wheel used to fall through to our history, which that screen does not
 * have, so a Codex tab stopped scrolling the moment its transcript opened.
 *
 * Exit code 0 when every check passed.
 */
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SRC = path.resolve(fileURLToPath(new URL('../..', import.meta.url)));
const TUI = path.join(SRC, 'tools', 'debug', 'alt-scroll-tui.mjs').replace(/\\/g, '/');
const LAB = process.env.SHIKISHA_WHEEL_LAB
  || (fs.existsSync('D:\\') ? 'D:\\ShikishaTerm-wheel' : path.join(os.tmpdir(), 'ShikishaTerm-wheel'));
const TOK = 'labtoken0123456789abcdef';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let fails = 0, checks = 0;
function ok(cond, what) {
  checks++;
  console.log((cond ? '    ok    ' : '    FAIL  ') + what);
  if (!cond) fails++;
}

// The board's port: the first free even one of 9400-9498, the range the
// debugging copies share, so this never lands on one of them
async function freePort() {
  for (let p = 9400; p < 9500; p += 2) {
    const free = await new Promise((res) => {
      const s = net.createServer().once('error', () => res(false))
        .once('listening', () => s.close(() => res(true))).listen(p, '127.0.0.1');
    });
    if (free) return p;
  }
  throw new Error('no free port in 9400-9499');
}

const ps = (cmd) => execFileSync('powershell', ['-NoProfile', '-Command', cmd], { encoding: 'utf8' }).trim();
function stopLab() {
  ps(`Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue |` +
     ` Where-Object { $_.Path -like '${LAB}\\*' } | Stop-Process -Force -ErrorAction SilentlyContinue`);
}

const TABS = [
  { id: 'w-keys', args: '' },
  { id: 'w-app', args: ' --app' },
  { id: 'w-quiet', args: ' --no-ask' },
];
const logOf = (id) => path.join(LAB, 'work', id + '.log');

function lay(port) {
  fs.rmSync(LAB, { recursive: true, force: true });
  for (const d of ['', 'config', 'logs', 'work']) fs.mkdirSync(path.join(LAB, d), { recursive: true });
  for (const n of ['SHIKISHA-TERM.exe', 'conpty.dll', 'OpenConsole.exe']) {
    const from = path.join(SRC, 'target', 'debug', n);
    if (fs.existsSync(from)) fs.copyFileSync(from, path.join(LAB, n));
  }
  for (const d of ['lang', 'profiles']) {
    if (fs.existsSync(path.join(SRC, d))) fs.cpSync(path.join(SRC, d), path.join(LAB, d), { recursive: true });
  }
  const cwd = path.join(LAB, 'work').replace(/\\/g, '/');
  fs.writeFileSync(path.join(LAB, 'config', 'config.json'), JSON.stringify({
    language: 'en', resident: false,
    remote: { enabled: true, bind: '127.0.0.1', port, sticky_token: true, fixed_token: TOK },
    desks: [{ id: 'default', name: 'DESK', folders: [{ name: 'w', cwd, tabs: TABS.map((t) => ({
      name: t.id, id: t.id, command: `node ${TUI} ${logOf(t.id).replace(/\\/g, '/')}${t.args}` })) }] }],
  }, null, 2), 'utf8');
}

async function main() {
  const port = await freePort();
  const base = `http://127.0.0.1:${port}`;
  console.log('lab: ' + LAB + ' (port ' + port + ')');
  stopLab();
  await sleep(1000);
  lay(port);
  spawn(path.join(LAB, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: LAB, detached: true, stdio: 'ignore' }).unref();
  let cookie = '';
  for (let i = 0; i < 30 && !cookie; i++) {
    await sleep(1000);
    try {
      const r = await fetch(`${base}/?t=${TOK}`);
      if (r.ok) cookie = (r.headers.getSetCookie ? r.headers.getSetCookie() : []).map((c) => c.split(';')[0]).join('; ') || 'none';
      await r.text();
    } catch (e) { /* not up yet */ }
  }
  if (!cookie) { console.log('the lab never came up'); return 2; }
  const say = async (body) => {
    await fetch(`${base}/api/intent?t=${TOK}`, {
      method: 'POST', headers: { 'content-type': 'application/json', cookie }, body: JSON.stringify(body), signal: AbortSignal.timeout(10000) }).then((r) => r.text());
    await sleep(1500);
  };
  // Every tab has to have started its program before the wheel means anything
  for (let i = 0; i < 20 && !TABS.every((t) => fs.existsSync(logOf(t.id))); i++) await sleep(500);
  const read = (id) => (fs.existsSync(logOf(id)) ? fs.readFileSync(logOf(id), 'latin1') : null);

  try {
    console.log('  asked for the wheel as keys');
    await say({ kind: 'select', tab: 1 });
    await say({ kind: 'scroll', by: 1, row: 2, col: 3 });
    ok(read('w-keys') === '\x1b[A'.repeat(3), 'one tick up is three up arrows: ' + JSON.stringify(read('w-keys')));
    fs.writeFileSync(logOf('w-keys'), '');
    await say({ kind: 'scroll', by: -2, row: 2, col: 3 });
    ok(read('w-keys') === '\x1b[B'.repeat(6), 'two ticks down are six down arrows: ' + JSON.stringify(read('w-keys')));

    console.log('  ...with application cursor keys');
    await say({ kind: 'select', tab: 2 });
    await say({ kind: 'scroll', by: 1, row: 0, col: 0 });
    ok(read('w-app') === '\x1bOA'.repeat(3), 'the arrows are the application form: ' + JSON.stringify(read('w-app')));

    console.log('  full screen, but did not ask');
    await say({ kind: 'select', tab: 3 });
    await say({ kind: 'scroll', by: 1, row: 0, col: 0 });
    ok(read('w-quiet') === '', 'nothing is typed into it: ' + JSON.stringify(read('w-quiet')));
  } finally {
    stopLab();
  }
  console.log(`\n${checks - fails}/${checks} passed`);
  return fails ? 1 : 0;
}

main().then((c) => process.exit(c), (e) => { console.error(e); stopLab(); process.exit(1); });
