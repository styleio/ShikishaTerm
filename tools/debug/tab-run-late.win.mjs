/**
 * A command given with `shikisha tab_run` that outlasts the caller's hold:
 * once it is over, the caller is told in its tab, at once.
 *
 * 2026-10-20: a 15-minute command's output reached its caller's inbox ten
 * minutes after the command had ended, and the caller was not told. Found
 * on the way: a caller on a desk not in front was never told of its mail
 * until its desk was shown again (--away).
 *
 *     cargo build
 *     node tools/debug/tab-run-late.win.mjs [--own] [--away] [--keep]
 *
 * Both tabs are Git Bash. The caller runs `shikisha tab_run` with its output
 * in a file; the command sleeps past the hold (the call prints STILL
 * WORKING), then prints a word of its own. The person names the runner in
 * the caller first, as the input bar sends it: a terminal is typed into only
 * once named. Checked: the call handed its
 * wait over, the word reached the inbox, and the line about new mail was
 * typed into the caller within seconds of the command's end.
 *
 * The terminals are held by the copy's own resident process, as they are by
 * default; --own leaves them to the app (keep_terminals off). --away shows
 * another desk once the call has handed its wait over, as a person who has
 * gone on to other work does: the two tabs are then on a desk not in front,
 * and the caller is told there all the same. Needs Windows,
 * Node 22 and Git for Windows. No AI, no account
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { connectCdp } from './chrome.mjs';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const KEEP = process.argv.includes('--keep');
const OWN = process.argv.includes('--own');
const AWAY = process.argv.includes('--away');
const RUN = fs.mkdtempSync(path.join(os.tmpdir(), 'sk-tabrun-'));
const INSTANCE = path.join(RUN, 'copy');
const APP = path.join(INSTANCE, 'app');
const WORK = path.join(RUN, 'work');
const CONFIG = path.join(RUN, 'config.json');
// Past the caller's hold (100 s), by a margin
const SLEEP = 130;
const CDP = 9000 + Math.floor(Math.random() * 900);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const launcher = path.join(ROOT, 'tools', 'debug', 'instance.win.ps1');
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8', windowsHide: true });
let cleaned = false;
const cleanup = () => {
  if (cleaned) return;
  cleaned = true;
  ps('-File', launcher, '-At', INSTANCE, '-Stop');
  if (KEEP) console.log('kept: ' + RUN);
};
process.on('exit', cleanup);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
{
  // A check of an older build says nothing about the code in front of you
  const built = fs.statSync(exe).mtimeMs;
  const newer = (dir) => fs.readdirSync(dir, { withFileTypes: true }).some((e) => {
    const p = path.join(dir, e.name);
    return e.isDirectory() ? newer(p) : fs.statSync(p).mtimeMs > built;
  });
  if (['src', path.join('crates', 'core', 'src')].some((d) => newer(path.join(ROOT, d)))) {
    die('the build at target\\debug is older than the sources -- run cargo build first');
  }
}
const bash = ['ProgramFiles', 'ProgramW6432'].map((k) => process.env[k]).filter(Boolean)
  .map((p) => path.join(p, 'Git', 'bin', 'bash.exe')).find((p) => fs.existsSync(p));
if (!bash) die('no Git for Windows under Program Files');

fs.mkdirSync(WORK, { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  ...(OWN ? { keep_terminals: false } : {}),
  external_api: { access: 'user' },
  desks: [{ name: 'Tab run', id: 'tab-run', folders: [
    { cwd: WORK, tabs: [
      { name: 'caller', id: 'caller', command: [bash, '--login', '-i'] },
      { name: 'runner', id: 'runner', command: [bash, '--login', '-i'] },
    ] },
  ] }, { name: 'Other', id: 'other', folders: [
    { cwd: WORK, tabs: [{ name: 'other', id: 'other', command: [bash, '--login', '-i'] }] },
  ] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
const started = spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', launcher,
  '-At', INSTANCE, '-Work', WORK, '-Exe', exe, '-Config', CONFIG, '-Cdp', String(CDP)],
  { env, encoding: 'utf8', windowsHide: true });
if (started.status !== 0) die('launch failed: ' + started.stdout + started.stderr);
const pid = Number(/^pid=(\d+)/m.exec(started.stdout)?.[1]);
if (!pid) die('the launcher did not identify its copy');
console.log(`isolated copy: ${RUN} (${OWN ? 'terminals of its own' : 'terminals held by its resident process'})`);

// ── The door: the pipe, as the person ─────────────────────────────────────
const dataDir = () => [path.join(APP, 'data'), path.join(RUN, 'localappdata', 'ShikishaTerm', 'data')]
  .find((d) => fs.existsSync(path.join(d, 'api-token')));
let door;
const openDoor = async () => {
  const end = Date.now() + 60000;
  let dir;
  while (!(dir = dataDir()) && Date.now() < end) await sleep(500);
  if (!dir) die('no api-token appeared');
  const sock = net.connect(`\\\\.\\pipe\\shikisha-${pid}`);
  await new Promise((r, j) => { sock.once('connect', r); sock.once('error', j); });
  let buf = '';
  const waiting = [];
  sock.on('data', (d) => {
    buf += d.toString('utf8');
    let i;
    while ((i = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, i); buf = buf.slice(i + 1);
      const w = waiting.shift(); if (w) w(JSON.parse(line));
    }
  });
  const line = (o) => new Promise((resolve) => { waiting.push(resolve); sock.write(JSON.stringify(o) + '\n'); });
  const hello = await line({ token: fs.readFileSync(path.join(dir, 'api-token'), 'utf8').trim() });
  if (!hello.ok) die('the door refused: ' + JSON.stringify(hello));
  let n = 0;
  door = async (method, ...params) => {
    const a = await line({ id: String(++n), method, params });
    if (!a.ok) throw new Error(`${method}: ${a.error}`);
    return a.result;
  };
};
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await test().catch(() => false)) return true;
    await sleep(250);
  }
  throw new Error('timed out waiting for ' + what);
};
const screen = (id) => door('tab_screen', id).then((s) => String(s || ''));
const hooksLog = () => {
  const f = path.join(APP, 'logs', 'hooks.log');
  return fs.existsSync(f) ? fs.readFileSync(f, 'utf8') : '';
};
const unix = (p) => p.replace(/\\/g, '/').replace(/^([A-Za-z]):/, (_, d) => '/' + d.toLowerCase());

try {
  await openDoor();
  for (const id of ['caller', 'runner']) await until(async () => /\$\s*$/m.test(await screen(id)), `the ${id}'s bash`, 60000);
  // Its shell at rest a while first, as a terminal in use is: what it holds
  // at rest is learned from a quiet moment once it has come up
  await sleep(20000);
  const word = 'RAN-' + Math.random().toString(36).slice(2, 8).toUpperCase();
  const ended = path.join(WORK, 'ended.txt');
  const out = path.join(WORK, 'call.txt');
  const back = path.join(WORK, 'call.end');
  // The command writes down when it is over, then says its word. A word of
  // nothing at each end: a bash that has not yet turned on bracketed paste
  // reads the paste's marks into the first and last words
  const command = `true; sleep ${SLEEP}; date +%s%3N > '${unix(ended)}'; echo ${word}; true`;
  fs.writeFileSync(path.join(WORK, 'command.txt'), command);
  const before = hooksLog().length;
  // Said by the person into the caller, as the input bar sends it, naming
  // the runner (a comment to bash)
  const pages = async () => (await (await fetch(`http://127.0.0.1:${CDP}/json/list`)).json()).filter((t) => t.type === 'page');
  let targets = [];
  await until(async () => (targets = await pages()).length > 0, 'the app\'s page', 30000);
  const board = await connectCdp(targets[0]);
  await until(async () => board.run('typeof S === "object" && !!S.tabs?.find(t => t.id === "caller")'), 'the board', 30000);
  // Said into the caller, and said again if the shell never started on it:
  // a line can go in while the shell is still taking the last one, and be
  // read with the paste's marks in it
  const say = async (text, started) => {
    for (let tries = 0; ; tries += 1) {
      const idx = await board.run('S.tabs.find(t => t.id === "caller").index');
      await board.run(`send({kind:"say", tab:${idx}, text:${JSON.stringify(text)}}); true`);
      try {
        return await until(async () => fs.existsSync(started), 'the caller to start on it', 15000);
      } catch (e) {
        if (tries >= 2) throw e;
      }
    }
  };
  await say(`shikisha tab_run runner "$(cat command.txt)" > '${unix(out)}' 2>&1; echo ok > '${unix(back)}' # <@runner>`, out);
  await until(async () => fs.existsSync(back), 'the call to hand its wait over', (SLEEP + 60) * 1000);
  const printed = fs.readFileSync(out, 'utf8');
  console.log('  the call printed:\n    ' + printed.trim().split('\n').slice(-2).join('\n    '));
  check(/STILL WORKING/.test(printed) && !printed.includes(word), 'the call handed its wait over before the command was done');
  // Gone on to other work: the desk in front is another one
  const deskShown = () => board.run('S.ui?.desk_index ?? S.desk_index ?? null');
  if (AWAY) {
    await board.run('send({kind:"runkey", name:"desk_next"}); true');
    await until(async () => (await screen('caller').then(() => false, () => true)) || (await deskShown()) === 1, 'the other desk to be shown', 20000);
    console.log('  the other desk is in front');
  }
  // When the ask was answered, as the log says it
  let answeredAt = 0;
  const watch = setInterval(() => { if (!answeredAt && /ask_tab: runner -> DONE/.test(hooksLog().slice(before))) answeredAt = Date.now(); }, 250);
  await until(async () => /^\d+\s*$/.test(fs.readFileSync(ended, 'utf8')), 'the command to end', (SLEEP + 30) * 1000);
  const endedAt = Number(fs.readFileSync(ended, 'utf8').trim());
  // Told by the line typed into it, read from its screen; on a desk not in
  // front, from the record of what was typed (the log)
  const told = async () => AWAY
    ? /orchestration: typed into caller/.test(hooksLog().slice(before))
    : /New mail for this tab/.test(await screen('caller'));
  let toldAt = 0;
  try {
    await until(told, 'the line about new mail', 90000);
    toldAt = Date.now();
  } catch (e) {
    console.log('  ' + e.message);
  }
  clearInterval(watch);
  // The last look, in case it came between two
  if (!answeredAt && /ask_tab: runner -> DONE/.test(hooksLog().slice(before))) answeredAt = toldAt || Date.now();
  const answered = answeredAt ? (answeredAt - endedAt) / 1000 : Infinity;
  check(answered < 10, `the ask was answered within seconds of the command's end (${answeredAt ? answered.toFixed(1) + ' s' : 'never'})`);
  const late = toldAt ? (toldAt - endedAt) / 1000 : Infinity;
  check(late < 15, `the caller was told within seconds of the command's end (${toldAt ? late.toFixed(1) + ' s' : 'never'})`);
  // Back to its desk: the line is there on its screen, typed while it was away
  if (AWAY) {
    await board.run('send({kind:"runkey", name:"desk_next"}); true');
    await until(async () => (await deskShown()) === 0, 'the desk to be shown again', 20000);
    let seen = false;
    try {
      await until(async () => /New mail for this tab/.test(await screen('caller')), 'the line on the caller\'s screen', 20000);
      seen = true;
    } catch {}
    check(seen, 'the line about new mail is on the caller\'s screen');
  }
  // The inbox read the way the AI reads it, from the caller's own shell
  const inboxOut = path.join(WORK, 'inbox.txt');
  const inboxEnd = path.join(WORK, 'inbox.end');
  let mail = '';
  try {
    await say(`shikisha inbox > '${unix(inboxOut)}' 2>&1; echo ok > '${unix(inboxEnd)}'`, inboxOut);
    await until(async () => fs.existsSync(inboxEnd), 'the caller\'s inbox', 30000);
    mail = fs.readFileSync(inboxOut, 'utf8');
  } catch {}
  const log = hooksLog().slice(before);
  check(mail.includes(word), 'its output is in the caller\'s inbox');
  try { board.ws.close(); } catch {}
  console.log('--- log ---\n' + log.split(/\r?\n/).filter((l) => /ask_tab|tab2|tab1|behind its prompt|far terminal/.test(l)).slice(-30).join('\n'));
} catch (e) {
  console.error(e.message || e);
  failures += 1;
  for (const id of ['caller', 'runner']) {
    try { console.log(`--- ${id} screen ---\n` + await screen(id)); } catch {}
  }
  console.log('--- log ---\n' + hooksLog().split(/\r?\n/).slice(-40).join('\n'));
}
console.log(failures ? `${failures} failed` : 'all passed');
cleanup();
process.exit(failures ? 1 : 0);
