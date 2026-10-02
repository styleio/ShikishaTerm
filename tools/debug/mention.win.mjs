/**
 * The @ in the input bar, end to end: typed, picked, painted, and sent as the
 * tab's id.
 *
 * This checkout's build in a folder of its own, with three tabs in one folder:
 * two stand-in AIs and a shell. The stand-in is a `claude.cmd` (so the app
 * takes it for Claude Code) running a script that writes down every byte it
 * is sent. Typing goes
 * through the window's own DevTools port as keystrokes, so the page sees what
 * a person's keyboard would give it.
 *
 *     cargo build
 *     node tools/debug/mention.win.mjs
 *
 * Checked: every tab finds `shikisha` on its PATH; the @ button is there over
 * an AI tab; the first time, the list asks about the skill before anything is
 * written and is the whole list, "Close" writes nothing and it comes back at
 * the next @, and "Install" writes
 * the skill and says so; typing "@" offers the other AI tab and not the shell
 * (and the terminal, which is given commands) and not the tab in front; Enter takes it and a badge is painted under it;
 * Backspace takes the whole badge back out; and what the tab in front
 * receives says `<@helper>`, not the name on the badge.
 *
 * The copy's Claude profile is pointed at a skills folder of its own (the
 * profiles are files beside the copy), so the skill is written there and
 * nothing in the real home is touched.
 *
 * Needs Windows and Node. No account is used and nothing leaves the machine.
 * Nothing of a copy somebody is using is read, written or stopped; the app is
 * stopped on the way out.
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const PORT = 9351;
const RUN = path.join(os.tmpdir(), 'sk-mention');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const STUB = path.join(RUN, 'stub');
const CONFIG = path.join(APP, 'config', 'config.json');
const HEARD = path.join(RUN, 'heard-front.txt');
const HOME = path.join(RUN, 'home');
const SKILL = path.join(HOME, '.claude', 'skills', 'shikisha', 'SKILL.md');
const PATHS = path.join(RUN, 'path-front.txt');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, STUB, HOME, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });

// The stand-in AI: shows a prompt, and writes down everything it is sent.
// A `claude.cmd` the way npm installs the real one, so the arguments the app
// adds for a new conversation (`--session-id …`) go to the script and not to
// Node, which would refuse them as options of its own. One folder per tab, so
// each writes down what it heard in a file of its own
fs.writeFileSync(path.join(STUB, 'listen.js'), [
  "const fs = require('fs');",
  'const out = process.argv[2];',
  // The PATH it was started with, for the check that `shikisha` is on it
  "fs.writeFileSync(out.replace(/heard-/, 'path-'), process.env.PATH || process.env.Path || '');",
  "if (process.stdin.isTTY) process.stdin.setRawMode(true);",
  "process.stdout.write('stand-in ready\\r\\n> ');",
  "process.stdin.on('data', (d) => { fs.appendFileSync(out, d); process.stdout.write('.'); });",
].join('\n'));
const standIn = (name, heard) => {
  const dir = path.join(STUB, name);
  fs.mkdirSync(dir, { recursive: true });
  const cmd = path.join(dir, 'claude.cmd');
  fs.writeFileSync(cmd, `@"${process.execPath}" "${path.join(STUB, 'listen.js')}" "${heard}" %*\r\n`);
  return [cmd];
};

const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
// Its Claude keeps skills here, not in the real home
const claudeProfile = path.join(APP, 'profiles', 'claude.json');
const prof = JSON.parse(fs.readFileSync(claudeProfile, 'utf8'));
prof.skills = path.join(HOME, '.claude', 'skills');
fs.writeFileSync(claudeProfile, JSON.stringify(prof, null, 2));
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'en',
  remote: { enabled: false },
  desks: [{ name: 'Mention', id: 'mention', folders: [{ cwd: WORK, tabs: [
    { name: 'front', id: 'front', command: standIn('front', HEARD) },
    { name: 'helper', id: 'helper', command: standIn('helper', path.join(RUN, 'heard-helper.txt')) },
    { name: 'shell', id: 'shell', command: 'cmd.exe' },
  ] }] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${PORT}`;
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

const pages = async () => {
  try { return (await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json()).filter((t) => t.type === 'page'); } catch { return []; }
};
const connect = async (target) => {
  const {ws, send, run} = await connectCdp(target);
  return { ws, send, run };
};
const until = async (test, what, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(250); }
  throw new Error('timed out waiting for ' + what);
};

let board;
try {
  let targets = [];
  await until(async () => (targets = await pages()).length > 0, 'the app\'s page', 30000);
  board = await connect(targets[0]);
  const { run, send } = board;
  const type = (text) => send('Input.insertText', { text });
  const key = (k, code, vk) => Promise.all([
    send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: k, code, windowsVirtualKeyCode: vk }),
    send('Input.dispatchKeyEvent', { type: 'keyUp', key: k, code, windowsVirtualKeyCode: vk }),
  ]);

  // The tab in front is the first stand-in, and the app has taken it for an AI
  await until(() => run(`!!(S.tabs || []).find(t => t.id === "front" && t.ai)`), 'the stand-in to be read as an AI', 30000);
  const front = await run(`S.tabs.find(t => t.id === "front").index`);
  await run(`send({kind:"select", tab:${front}}); true`);
  await until(() => run(`S.active === ${front}`), 'the stand-in in front');
  await sleep(1500);

  console.log('0. every tab finds shikisha on its PATH');
  await until(async () => fs.existsSync(PATHS), 'the stand-in to write down its PATH', 15000);
  const bin = fs.readFileSync(PATHS, 'utf8').split(';')[0];
  check(fs.existsSync(path.join(bin, 'shikisha.cmd')) && fs.existsSync(path.join(bin, 'shikisha')),
    'the first folder on the PATH holds shikisha.cmd and shikisha: ' + bin);

  console.log('1. the @ button, over an AI tab');
  await run(`showDock(); castInput.focus(); true`);
  check(await run(`castMentionEl && castMentionEl.style.display !== "none"`), 'the @ button is shown');

  console.log('1a. the first time, the skill is asked about before anything is written');
  await run(`castMentionEl.click(); true`);
  await until(() => run(`!!document.querySelector(".fmenu.mentions .mskill .go")`), 'the card', 5000);
  check(await run(`document.querySelector(".fmenu.mentions .mskill code").textContent`) === SKILL,
    'the card says where it would write: ' + SKILL);
  check(await run(`document.querySelectorAll(".fmenu.mentions .mrow").length`) === 0, 'the card is the whole list: no tabs under it');
  check(!fs.existsSync(SKILL), 'nothing is written yet');
  await run(`[...document.querySelectorAll(".fmenu.mentions .mskill button")].find(b => b.classList.contains("quiet")).click(); true`);
  await until(() => run(`!document.querySelector(".fmenu.mentions")`), 'the list to close', 5000);
  check(!fs.existsSync(SKILL), '"Close" writes nothing');
  await run(`castMentionEl.click(); true`);
  await until(() => run(`!!document.querySelector(".fmenu.mentions .mskill .go")`), 'the card again', 5000);
  check(true, 'the next @ asks again');
  await run(`document.querySelector(".fmenu.mentions .mskill button.primary").click(); true`);
  await until(async () => fs.existsSync(SKILL), 'the skill to be written', 10000);
  check(fs.readFileSync(SKILL, 'utf8').startsWith('---\nname: shikisha'), '"Install" writes the skill');
  await until(() => run(`!!document.querySelector(".fmenu.mentions .mskill.done")`), 'the line saying it is in', 10000);
  check(true, 'the list says it is in');
  await run(`closeMentions(); castInput.value = ""; growCastInput(); castInput.focus(); true`);
  await sleep(300);

  console.log('2. typing @ offers the other tabs, and not the tab in front');
  await type('Ask ');
  await type('@');
  await until(() => run(`!!document.querySelector(".fmenu.mentions")`), 'the list', 5000);
  const offered = await run(`[...document.querySelectorAll(".fmenu.mentions .mrow .nm")].map(e => e.textContent)`);
  check(JSON.stringify(offered) === JSON.stringify(['helper', 'shell']), 'offered: ' + JSON.stringify(offered));

  console.log('3. Enter takes it, and a badge is painted under it');
  await key('Enter', 'Enter', 13);
  await sleep(200);
  check(await run(`castInput.value`) === 'Ask @helper ', 'the text reads ' + JSON.stringify(await run(`castInput.value`)));
  check(await run(`!document.querySelector(".fmenu.mentions")`), 'the list is put away');
  check(await run(`[...castMirror.querySelectorAll("mark")].map(m => m.textContent).join()`) === '@helper', 'one badge, on @helper');

  console.log('4. Backspace takes the whole badge back out');
  await key('Backspace', 'Backspace', 8);   // the space after it
  await key('Backspace', 'Backspace', 8);   // into the badge
  await sleep(200);
  check(await run(`castInput.value`) === 'Ask ', 'the text reads ' + JSON.stringify(await run(`castInput.value`)));
  check(await run(`castMirror.querySelectorAll("mark").length`) === 0, 'no badge is left');

  console.log('5. sent, it names the tab by its id');
  await type('@help');   // not @hel: "shell" has that in it too
  await until(() => run(`document.querySelectorAll(".fmenu.mentions .mrow").length === 1`), 'the narrowed list', 5000);
  await key('Enter', 'Enter', 13);
  await type('to review this');
  await sleep(200);
  await key('Enter', 'Enter', 13);
  await until(async () => fs.existsSync(HEARD) && fs.readFileSync(HEARD, 'utf8').includes('review this'), 'the stand-in to be sent the line', 15000);
  const heard = fs.readFileSync(HEARD, 'utf8');
  check(heard.includes('Ask <@helper> to review this'), 'the tab in front was sent ' + JSON.stringify(heard.replace(/\x1b\[20[01]~/g, '')));
} catch (e) {
  failures += 1;
  console.error('stopped: ' + e.message);
} finally {
  try { board?.ws.close(); } catch {}
  // --keep leaves the copy up to look at
  if (!process.argv.includes('--keep')) stopApp();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
