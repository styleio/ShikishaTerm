/**
 * An ask_tab answered while its desk is not in front.
 *
 * ask_tab waits on the tab it asked; a person switching to another desk
 * meanwhile used to make the tab look gone, and the answer came back as
 * "GONE: the tab was closed" for a tab that was running all along. This asks
 * a real Codex a short question through the pipe (as the person), switches to
 * the other desk the moment the words have gone in -- with the same key a
 * person presses, sent through the board's intent door -- and checks the
 * answer that comes back on the held line is the real one.
 *
 *     cargo build
 *     node tools/debug/ask-tab-desk.win.mjs [--runs=2]
 *
 * Needs Windows, Node and `codex` signed in on this machine. Spends one short
 * turn of Codex a run. Isolated the way ask-tab-poc.win.mjs is: its own folder
 * and LOCALAPPDATA, nothing of a copy somebody is using touched.
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
// A folder of its own each run: a browser process of the last run can still
// hold a file in the old one for a while after the app is gone
const RUN = path.join(os.tmpdir(), `sk-askdesk-${Date.now().toString(36)}`);
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const CONFIG = path.join(APP, 'config', 'config.json');
const HOOKS_LOG = path.join(APP, 'logs', 'hooks.log');
const RUNS = Number((process.argv.find((a) => a.startsWith('--runs=')) || '--runs=2').slice(7));
const TOKEN = 'askdesk-check-token-0123456789';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
const nonce = () => Math.random().toString(36).slice(2, 8).toUpperCase();
const freePort = () => new Promise((r) => { const s = net.createServer(); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => r(p)); }); });

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
if (spawnSync('where.exe', ['codex'], { encoding: 'utf8' }).status !== 0) die('codex is not on PATH');

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
const folders = { a: path.join(WORK, 'desk-a'), b: path.join(WORK, 'desk-b') };
for (const d of [APP, ...Object.values(folders), path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });
ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed');
spawnSync('git', ['init', '-q'], { cwd: folders.a });

const port = await freePort();
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  remote: { enabled: true, bind: '127.0.0.1', port, sticky_token: true, fixed_token: TOKEN },
  external_api: { access: 'user' },
  desks: [
    { name: 'First', id: 'first', folders: [{ cwd: folders.a, tabs: [{ name: 'codex-a', id: 'codex-a', command: ['codex', '--dangerously-bypass-approvals-and-sandbox'] }] }] },
    { name: 'Second', id: 'second', folders: [{ cwd: folders.b, tabs: [{ name: 'shell-b', id: 'shell-b', command: ['cmd.exe'] }] }] },
  ],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
const child = spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' });
const pid = child.pid;
child.unref();

// ── The pipe, as the person; each call on a connection of its own, so an
//    ask held open does not hold up the rest ─────────────────────────────────
const tokenFile = () => [path.join(APP, 'data', 'api-token'), path.join(RUN, 'localappdata', 'ShikishaTerm', 'data', 'api-token')].find((p) => fs.existsSync(p));
const call = async (method, ...params) => {
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
  const hello = await line({ token: fs.readFileSync(tokenFile(), 'utf8').trim() });
  if (!hello.ok) throw new Error('the door refused');
  const a = await line({ id: '1', method, params });
  sock.destroy();
  if (!a.ok) throw new Error(`${method}: ${a.error}`);
  return a.result;
};
// The board takes a press only from a device that opened it: the page is
// opened once, the way a phone does, and its cookies go with every press
let cookies = '';
const openBoard = async () => {
  const r = await fetch(`http://127.0.0.1:${port}/?t=${TOKEN}`, { redirect: 'manual', signal: AbortSignal.timeout(10000) });
  cookies = r.headers.getSetCookie().map((c) => c.split(';')[0]).join('; ');
  if (!/(^|; )rs=/.test(cookies)) throw new Error(`the board gave no session (${r.status}): ${cookies}`);
};
const intent = async (body) => {
  if (!cookies) await openBoard();
  const r = await fetch(`http://127.0.0.1:${port}/api/intent?t=${TOKEN}`, { method: 'POST', body: JSON.stringify(body), headers: { Cookie: cookies }, signal: AbortSignal.timeout(10000) });
  const text = await r.text();
  try { return JSON.parse(text); } catch { throw new Error(`the board answered ${r.status}: ${text.slice(0, 200)}`); }
};
const logLines = () => (fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8').split(/\r?\n/) : []);
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return; await sleep(500); }
  throw new Error('timed out waiting for ' + what);
};

let pass = 0;
try {
  await until(async () => tokenFile(), 'the api token', 60000);
  await until(async () => (await call('state', 'codex-a')) !== undefined, 'the Codex tab', 60000);
  // Up to its prompt, past a folder-trust question if one comes
  await until(async () => {
    const s = String(await call('tab_screen', 'codex-a'));
    if (/Skip until next version/i.test(s)) { await call('send', 'codex-a', '\x1b[B'); await sleep(300); await call('send', 'codex-a', '\r'); await sleep(2000); return false; }
    if (/trust|allow Codex to work/i.test(s) && !/Ask Codex/i.test(s)) { await call('send', 'codex-a', '\r'); return false; }
    return /Ask Codex to do anything|context left|esc to interrupt/i.test(s);
  }, 'Codex to come up', 120000);
  console.log('Codex is up');
  // Its prompt is drawn before it has finished starting; a question typed in
  // that moment is taken late. The board opened now, too, not mid-run
  await sleep(8000);
  await openBoard();

  if (process.argv.includes('--probe')) {
    await sleep(8000);
    console.log('state before:', await call('state', 'codex-a'));
    const r = await call('ask_tab', 'codex-a', 'Reply with only the word PROBE1 and nothing else. Do not use any tools.');
    console.log('probe answer:', JSON.stringify(r).slice(0, 200));
    for (let i = 0; i < 6; i++) { console.log('state:', await call('state', 'codex-a')); await sleep(5000); }
    const sc = String(await call('tab_screen', 'codex-a')).split('\n').filter((l) => l.trim()).slice(-8).join('\n');
    console.log('screen:\n' + sc);
  }
  // Which desk is in front, as the pipe sees it: the tabs it lists
  const front = async () => {
    const list = await call('tab_list');
    const ids = (Array.isArray(list) ? list : []).map((t) => t.id);
    return ids.includes('shell-b') ? 'Second' : ids.includes('codex-a') ? 'First' : `? ${JSON.stringify(ids)}`;
  };
  const switchTo = async (want) => {
    const went = await intent({ kind: 'runkey', name: 'desk_next' });
    if (!went.ok) throw new Error('the board did not take the key');
    await until(async () => (await front()) === want, `the ${want} desk in front`, 15000);
  };
  // The switch itself, before anything depends on it
  await switchTo('Second');
  await switchTo('First');
  console.log('switching desks works');

  for (let n = 1; n <= (process.argv.includes('--probe') ? 0 : RUNS); n++) {
    const word = `DESK${nonce()}`;
    const from = logLines().length;
    // A question that takes a while, so the answer comes while the desk is away
    const asked = call('ask_tab', 'codex-a',
      `Run a shell command that waits 20 seconds, then reply with only the word ${word} and nothing else.`);
    // Away the moment the words have gone in
    await until(async () => logLines().slice(from).some((l) => l.includes('ask_tab: sent to codex-a')), 'the words to go in', 60000);
    await switchTo('Second');
    const awayAt = Date.now();
    const r = await asked;
    const answeredAway = (await front()) === 'Second';
    const ok = r && r.state === 'DONE' && String(r.reply || '').includes(word);
    console.log(`${ok ? 'PASS' : 'FAIL'} #${n}: answered ${Math.round((Date.now() - awayAt) / 1000)}s after the switch, ` +
      `desk in front at the answer: ${answeredAway ? 'Second (away)' : 'First'}; state=${r && r.state} ` +
      `reply=${JSON.stringify(String((r && (r.reply || r.note)) || '').slice(0, 80))}`);
    if (ok && answeredAway) pass++;
    else {
      // Looked at from its own desk: the pipe reads only the desk in front
      await switchTo('First');
      const s = await call('tab_screen', 'codex-a').catch((e) => `(no screen: ${e.message})`);
      console.log(`  codex-a is ${await call('state', 'codex-a').catch((e) => e.message)}; its screen ends:\n` +
        String(s).split('\n').filter((l) => l.trim()).slice(-8).map((l) => '    ' + l).join('\n'));
    }
    if ((await front()) !== 'First') await switchTo('First');
    await sleep(3000);
  }
} catch (e) {
  console.error('stopped: ' + e.message);
  const s = await call('tab_screen', 'codex-a').catch(() => '(no screen)');
  console.error('--- codex-a screen ---\n' + String(s).split('\n').filter((l) => l.trim()).slice(-15).join('\n'));
} finally {
  console.log(`all: ${pass}/${RUNS}`);
  for (const l of logLines().filter((l) => l.includes('ask_tab:')).slice(-8)) console.log('  ' + l);
  if (!process.argv.includes('--keep')) stopApp();
}
