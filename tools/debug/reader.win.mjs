/**
 * The reader (📖) in a running copy, on the window and on a phone.
 *
 * A full-screen AI CLI keeps no scrollback, so what has scrolled out of its
 * screen cannot be selected on the terminal. The reader shows the conversation
 * from the CLI's own record as a document instead, behind one button in the
 * same place on both: this checks that it is there on an AI tab and nowhere
 * else, that it opens over the pane it reads on the window and over the whole
 * screen on a phone, that what it reads comes down the conversation line on
 * both (the phone's over its socket), that what is selected in it is copied,
 * and that Esc puts it away and gives the terminal its keys back.
 *
 *     cargo build
 *     node tools/debug/reader.win.mjs
 *
 * Needs Windows, Node and Chrome. Started through instance.win.ps1 with a home
 * of its own and a stand-in `claude` on its PATH -- no account is used and
 * nothing leaves the machine -- and the conversation written the way Claude
 * Code writes one. The clipboard is the machine's: what was on it is put back.
 * Pictures go to target/shots/reader-*.png. The copy is stopped on the way out.
 */
import {connectCdp, startChrome} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {spawnSync} from 'node:child_process';
import {randomUUID} from 'node:crypto';

const root = path.resolve(import.meta.dirname, '../..');
const lab = fs.mkdtempSync(path.join(os.tmpdir(), 'sk-reader-'));
const instance = path.join(lab, 'instance');
const home = path.join(lab, 'home');
const stub = path.join(lab, 'stub');
const argvFile = path.join(lab, 'argv.txt');
const launch = path.join(root, 'tools/debug/instance.win.ps1');
const shots = path.join(root, 'target/shots');
const token = randomUUID();
// A home with no AppData in it is one WebView2 will not start in: the window
// opens with no page and no DevTools port
for (const d of [path.join(home, 'AppData', 'Roaming'), path.join(home, 'AppData', 'Local'), stub, shots]) {
  fs.mkdirSync(d, {recursive: true});
}

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const quote = value => "'" + value.replaceAll("'", "''") + "'";
const ps = (command, env) => {
  const r = spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', command],
    {encoding: 'utf8', windowsHide: true, env: env || process.env});
  if (r.status !== 0) throw new Error(r.stdout + r.stderr);
  return r.stdout;
};
async function until(f, what, ms = 30000) {
  const end = Date.now() + ms;
  let last;
  while (Date.now() < end) {
    try { last = await f(); if (last) return last; } catch (e) { last = e.message; }
    await sleep(200);
  }
  throw new Error(`timed out waiting for ${what} (${JSON.stringify(last)})`);
}
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };

// A stand-in for Claude Code: says how it was started, then waits
fs.writeFileSync(path.join(stub, 'claude.cmd'),
  '@echo off\r\n' + `echo %*> "${argvFile}"\r\n` + ':wait\r\n' + 'timeout /t 60 /nobreak >nul\r\n' + 'goto wait\r\n');

// The conversation, the way Claude Code writes it: a question, a tool's work,
// and an answer long enough that its head would have left a terminal's screen
const ANSWER_HEAD = 'The checkout page was slow because the cart total read every tax band on each request.';
const answer = [ANSWER_HEAD, '',
  ...Array.from({length: 60}, (_, i) => `- Step ${i + 1}: a line of the explanation that a terminal would scroll away.`),
  '', '```rust', 'let bands = cache.bands_or(|| db.tax_bands())?;', '```'].join('\n');
const record = (cwd) => [
  {type: 'user', cwd, message: {role: 'user', content: 'the checkout page is slow, find out why'}},
  {type: 'assistant', cwd, message: {role: 'assistant', content: [{type: 'tool_use', id: 'toolu_1', name: 'Bash', input: {command: 'cargo test'}}]}},
  {type: 'user', cwd, message: {role: 'user', content: [{type: 'tool_result', tool_use_id: 'toolu_1', content: 'ok'}]}},
  {type: 'assistant', cwd, message: {role: 'assistant', content: [{type: 'text', text: answer}]}},
].map(l => JSON.stringify(l)).join('\n') + '\n';

const config = path.join(lab, 'config.json');
fs.writeFileSync(config, JSON.stringify({
  language: 'en', resident: false,
  agent_hooks: {'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off'},
  remote: {sticky_token: true, fixed_token: token},
  desks: [{id: 'check', name: 'Reader', folders: [{cwd: '{work}', tabs: [
    {id: 'coder', name: 'coder', command: 'claude'},
    {id: 'shell', name: 'shell', command: 'cmd.exe'},
  ]}]}],
}));

// The clipboard as it was, to be put back. Carried as base64 both ways: the
// console's own code page turns anything past ASCII into "?", and what was
// put back would no longer be what was there
const clipRead = () => Buffer.from(
  ps('[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes("$(Get-Clipboard -Raw)"))').trim(), 'base64').toString('utf8');
const clipBefore = clipRead();
const clipboard = () => clipRead().replace(/\r?\n$/, '');

let local, phone;
try {
  const env = {...process.env, USERPROFILE: home, HOME: home, PATH: stub + ';' + process.env.PATH};
  const started = ps(`& ${quote(launch)} -At ${quote(instance)} -Config ${quote(config)}`, env);
  const board = /^board=(.+)$/m.exec(started)[1].trim();
  const cdp = /^cdp=(.+)$/m.exec(started)[1].trim();
  const work = /^root=(.+)$/m.exec(started) ? path.join(instance, 'work') : '';
  const target = await until(async () => (await (await fetch(cdp + '/json/list')).json()).find(t => t.type === 'page'), 'the window');
  local = await connectCdp(target, {timeout: 30000});
  const run = local.run;
  await until(() => run('typeof S !== "undefined" && !!S && S.tabs.some(t => t.id === "coder")'), 'the board');

  // The conversation the app started the stand-in on, written down
  const argv = await until(() => fs.existsSync(argvFile) && fs.readFileSync(argvFile, 'utf8'), 'claude to be started');
  const id = /--session-id\s+(\S+)/.exec(argv)?.[1];
  check(!!id, 'the app started claude on a conversation of its own: ' + argv.trim());
  const projects = path.join(home, '.claude', 'projects', 'work');
  fs.mkdirSync(projects, {recursive: true});
  fs.writeFileSync(path.join(projects, id + '.jsonl'), record(work));

  const select = (client, tabId) => client.run(`send({kind:"select", tab:S.tabs.find(t => t.id === ${JSON.stringify(tabId)}).index}); true`);
  const shown = (client, elId) => client.run(`(() => { const e = document.getElementById(${JSON.stringify(elId)});
    return !!e && getComputedStyle(e).display !== "none" && e.getBoundingClientRect().width > 0; })()`);

  console.log('1. the window: 📖 on the AI tab alone, and no pager');
  await select(local, 'coder');
  await until(() => run('S.tabs[S.tabs.findIndex(t => t.index === S.active)].id === "coder"'), 'the AI tab in front');
  check(await until(() => shown(local, 'readOpen'), '📖'), '📖 stands on the AI tab');
  check(!(await shown(local, 'pageUp')), 'the phone pager is not on the window');

  console.log('2. opened, it reads the conversation over the pane it stands on');
  await run('document.getElementById("readOpen").click(); true');
  await until(() => run('document.querySelectorAll("#rbody .rturn").length >= 2'), 'the conversation');
  const read = await run(`(() => {
    const r = document.getElementById("reader").getBoundingClientRect(), s = document.getElementById("screen").getBoundingClientRect();
    return { turns: [...document.querySelectorAll("#rbody .rturn")].map(t => t.className),
      head: (document.querySelector("#rbody .rturn.ai p") || {}).textContent || "",
      over: Math.abs(r.left - s.left) < 1 && Math.abs(r.right - s.right) < 1 && Math.abs(r.top - s.top) < 1,
      keys: document.activeElement && document.activeElement.id };
  })()`);
  check(read.turns.join() === 'rturn you,rturn ai', 'what was said, oldest first, the work left out: ' + read.turns.join(' | '));
  check(read.head === ANSWER_HEAD, 'the head of the long answer is there: ' + read.head);
  check(read.over, 'it stands over the pane, not the window');
  check(read.keys === 'rbody', 'the keys are the document\'s, not the terminal\'s: ' + read.keys);
  let picture = await local.send('Page.captureScreenshot', {format: 'png'});
  fs.writeFileSync(path.join(shots, 'reader-window.png'), Buffer.from(picture.data, 'base64'));

  console.log('3. what is selected in it is copied');
  await run(`(() => { const p = document.querySelector("#rbody .rturn.ai p"); const range = document.createRange();
    range.selectNodeContents(p); const sel = getSelection(); sel.removeAllRanges(); sel.addRange(range);
    p.dispatchEvent(new MouseEvent("mouseup", {bubbles: true})); return true; })()`);
  check(await until(() => clipboard() === ANSWER_HEAD, 'the clipboard', 8000).catch(() => false), 'the selection is on the clipboard');
  // A list is copied as it is laid out: the bullets the screen draws with it
  await run(`(() => { const items = document.querySelectorAll("#rbody .rturn.ai li"); const range = document.createRange();
    range.setStartBefore(items[0]); range.setEndAfter(items[1]); const sel = getSelection(); sel.removeAllRanges(); sel.addRange(range);
    items[1].dispatchEvent(new MouseEvent("mouseup", {bubbles: true})); return true; })()`);
  const bullets = [1, 2].map(i => `• Step ${i}: a line of the explanation that a terminal would scroll away.`).join('\n');
  check(await until(() => clipboard().replace(/\r\n/g, '\n') === bullets, 'the clipboard', 8000).catch(() => false),
    'a list is on the clipboard with its bullets');

  console.log('3b. while it is up, the keys stay its own');
  await run('window.dispatchEvent(new Event("focus")); true');
  check(await run('document.activeElement && document.activeElement.id === "rbody"'),
    'the window coming back to the front does not hand the keys to the terminal');
  await run('window.__openPalette("files"); true');
  const palette = await until(() => run(`(() => { const a = document.activeElement;
    return a && a.tagName === "INPUT" && a.closest("#reader") === null ? a.id || a.className || "input" : ""; })()`), 'the palette');
  await run(`document.activeElement.dispatchEvent(new KeyboardEvent("keydown", {key: "Escape", bubbles: true, cancelable: true})); true`);
  await sleep(200);
  check(await run('rdIsOpen()'), 'Esc in the palette (' + palette + ') leaves the reader open');
  await run('if (typeof closePalette === "function") closePalette(); true');
  await run('rdBodyEl().focus(); true');
  check(!(await run(`(() => { const e = document.getElementById("side"); return !!e && getComputedStyle(e).zIndex !== "auto"
    && Number(getComputedStyle(e).zIndex) <= Number(getComputedStyle(document.getElementById("reader")).zIndex); })()`)),
    'the right-hand column stands over the reader, not under it');

  console.log('4. Esc puts it away and gives the terminal its keys back');
  await run('rdBodyEl().dispatchEvent(new KeyboardEvent("keydown", {key: "Escape", bubbles: true, cancelable: true})); true');
  check(!(await run('rdIsOpen()')), 'the reader is put away');
  check(await run('document.activeElement === kbd'), 'the caret is back in the terminal');

  console.log('5. a shell has no 📖');
  await select(local, 'shell');
  await until(() => run('S.tabs.find(t => t.index === S.active).id === "shell"'), 'the shell in front');
  await sleep(300);
  check(!(await shown(local, 'readOpen')), 'no 📖 over a shell');
  await select(local, 'coder');

  console.log('6. a phone: 📖 above the pager, the whole screen, read over its socket');
  phone = await startChrome();
  await phone.send('Page.enable');
  await phone.send('Emulation.setDeviceMetricsOverride', {width: 390, height: 844, deviceScaleFactor: 3, mobile: true});
  await phone.send('Emulation.setTouchEmulationEnabled', {enabled: true, maxTouchPoints: 5});
  await phone.send('Page.navigate', {url: board + '/?t=' + token});
  await until(() => phone.run('typeof S !== "undefined" && !!S && S.tabs.some(t => t.id === "coder")'), 'the phone board');
  await select(phone, 'coder');
  check(await until(() => shown(phone, 'readOpen'), '📖 on the phone'), '📖 stands on the phone\'s AI tab');
  check(await shown(phone, 'pageUp'), 'and the pager under it');
  await phone.run('document.getElementById("readOpen").click(); true');
  await until(() => phone.run('document.querySelectorAll("#rbody .rturn").length >= 2'), 'the conversation on the phone');
  const far = await phone.run(`(() => { const r = document.getElementById("reader").getBoundingClientRect();
    return { full: r.left === 0 && r.top === 0 && r.right === innerWidth && r.bottom === innerHeight,
      head: (document.querySelector("#rbody .rturn.ai p") || {}).textContent || "" }; })()`);
  check(far.full, 'it takes the whole phone screen');
  check(far.head === ANSWER_HEAD, 'the same words arrive on the phone');
  picture = await phone.send('Page.captureScreenshot', {format: 'png'});
  fs.writeFileSync(path.join(shots, 'reader-phone.png'), Buffer.from(picture.data, 'base64'));
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + e.message);
} finally {
  try { phone?.stop(); } catch {}
  try { local?.ws?.close(); } catch {}
  try { ps(`& ${quote(launch)} -At ${quote(instance)} -Stop`); } catch (e) { console.error(e.message); }
  try {
    ps('Set-Clipboard -Value ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($env:CLIP_BACK)))',
      {...process.env, CLIP_BACK: Buffer.from(clipBefore.replace(/\r?\n$/, ''), 'utf8').toString('base64')});
  } catch {}
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
