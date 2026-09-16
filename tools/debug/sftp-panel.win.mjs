/**
 * The file panel, checked through the running app's own window.
 *
 * Everything below the screen has tests, and they did not catch three faults a
 * person meets at once: a folder refused on a desk with no automation, "done"
 * wiped from the screen a few milliseconds after it was written, and a folder
 * held still by the stop button. Each lived in the join between the window and
 * the runtime, which is the part no test stands in.
 *
 * So this starts the app built in this checkout, in a folder of its own, with
 * one file panel pointed at a real OpenSSH server in WSL, and presses its
 * buttons -- real pointer events into the real page, through its WebView2's
 * DevTools port. Every step is checked against the files themselves, on the
 * server and on this PC, and not only against what the window says.
 *
 *     cargo build
 *     node tools/debug/sftp-panel.win.mjs
 *
 * Needs Windows, WSL (Ubuntu, with apt) and Node. Isolated the way a new-user
 * run is: its own folder, its own LOCALAPPDATA (the WebView2 store lives there),
 * and started from its own folder (settings are also looked for relative to
 * where the app starts). Nothing of a copy somebody is using is read, written
 * or stopped. Photographs land in target/shots; the app and the server are
 * stopped on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const DISTRO = process.env.SHIKISHA_WSL_DISTRO || 'Ubuntu';
const PORT = 9341;
// Short, under the temporary folder: the WebView2 store nests deeply, and a
// long base path runs past what Windows allows
const RUN = path.join(os.tmpdir(), 'sk-check');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };

// --exec, so nothing is read by a shell on the way: after `--` a Windows path
// loses its backslashes to the Linux shell before the command ever sees it
const wsl = (...args) => spawnSync('wsl.exe', ['-d', DISTRO, '--exec', ...args], { encoding: 'utf8' });
const toWsl = (p) => wsl('wslpath', '-a', p.split(path.sep).join('/')).stdout.trim();
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });

/** This checkout's copy, stopped by where it runs from and nothing else */
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);

// --- the server ------------------------------------------------------------------
const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
console.log('starting a real OpenSSH server in WSL');
const script = toWsl(path.join(ROOT, 'tools', 'debug', 'sftp-server.wsl.sh'));
if (!script) die('WSL could not say where this checkout is -- is the ' + DISTRO + ' distro installed?');
const started = spawnSync('wsl.exe', ['-d', DISTRO, '--cd', '~', '--exec', 'bash', script], { encoding: 'utf8' });
const said = Object.fromEntries((started.stdout || '').split('\n').filter((l) => l.includes('=')).map((l) => l.trim().split('=')));
if (said.listening !== 'yes') die('the server did not start:\n' + started.stdout + started.stderr);
const SERVE = `${said.serve}/site`;
const onServer = (rel) => { const r = wsl('cat', `${SERVE}/${rel}`); return r.status === 0 ? r.stdout : null; };
const waitServer = async (rel, want, ms = 30000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (onServer(rel) === want) return true; await sleep(300); }
  return false;
};

// --- the app, on its own --------------------------------------------------------------
console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, path.join(RUN, 'localappdata'), path.join(RUN, 'key')]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);

// Written with LF, the way the server's files are, so a compare shows the one
// line that differs and not every line's ending
const put = (rel, text) => { const p = path.join(WORK, rel); fs.mkdirSync(path.dirname(p), { recursive: true }); fs.writeFileSync(p, text); };
put('notes.txt', 'one\nTWO\nthree\n');
put('new.txt', 'made on this PC\n');
put('dist/top.txt', 'top');
put('dist/a/b/deep.txt', 'deep');

// The key, byte for byte: through a Windows text tool it gains CRLF and is refused
const key = path.join(RUN, 'key', 'client_key');
wsl('cp', said.key, toWsl(key));
if (!fs.existsSync(key)) die('the key was not copied');

fs.mkdirSync(path.join(APP, 'config'), { recursive: true });
fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
  language: 'ja',
  remote: { enabled: false },
  desks: [{
    name: 'Check', id: 'check',
    folders: [{
      cwd: WORK,
      tabs: [{
        name: 'deploy', id: 'deploy',
        command: `sftp://${said.user}@127.0.0.1:2222`,
        server: { key, remote_dir: SERVE },
      }],
    }],
  }],
}, null, 2));

// Started directly, so the isolated LOCALAPPDATA and the DevTools port reach
// it. What says "you are inside Claude Code" is taken out, so the app is not
// told it is somebody's child session
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${PORT}`;
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

// --- its page ------------------------------------------------------------------------
let targets;
for (let i = 0; i < 80 && !targets; i++) {
  try { targets = (await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json()).filter((t) => t.type === 'page'); } catch { await sleep(250); }
  if (targets && !targets.length) targets = null;
}
if (!targets) { stopApp(); die('the app\'s page never opened its DevTools port'); }
const ws = new WebSocket(targets[0].webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r, { once: true }));
let id = 0;
const waiting = new Map();
ws.addEventListener('message', (e) => {
  const m = JSON.parse(e.data);
  if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
});
const send = (method, params = {}) => new Promise((res, rej) => {
  const n = ++id;
  waiting.set(n, (m) => (m.error ? rej(new Error(method + ': ' + JSON.stringify(m.error))) : res(m.result)));
  ws.send(JSON.stringify({ id: n, method, params }));
});
const run = async (expression) => {
  const r = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
  return r.result.value;
};
const until = async (expression, what, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await run(expression).catch(() => false)) return true; await sleep(150); }
  throw new Error('timed out waiting for ' + what);
};
// A real click at the middle of whatever `find` points at, found again right
// before the click: the panel rebuilds its rows several times a second
const click = async (find, what) => {
  const at = await run(`(() => { const e = (${find})(); if (!e) return null;
    const r = e.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
  if (!at) throw new Error('nothing to click: ' + what);
  for (const type of ['mouseMoved', 'mousePressed', 'mouseReleased']) {
    await send('Input.dispatchMouseEvent', { type, x: at.x, y: at.y, button: 'left', clickCount: 1 });
  }
  await sleep(250);
};
const shot = (name) => ps('-File', path.join(ROOT, 'tools', 'debug', 'shot-window.win.ps1'),
  '-Under', RUN, '-Out', path.join(SHOTS, `sftp-panel-${name}.png`));

// Found in the page's own terms, so a change of wording does not break them
const row = (side, name) => `() => [...sftpUi.${side}.list.querySelectorAll('.frow')].find(r => r.querySelector('.nm')?.textContent === ${JSON.stringify(name)})`;
const more = (side, name) => `() => (${row(side, name)})()?.querySelector('.more')`;
const menuItem = (key) => `() => [...document.querySelectorAll('div')].filter(d => d.offsetParent && d.children.length === 0 && d.textContent === T[${JSON.stringify(key)}]).pop()`;
const sendButton = (side) => `() => [...sftpUi.${side}.acts.querySelectorAll('button')].find(b => b.classList.contains('go'))`;
const tick = async (side, name) => {
  await click(row(side, name), 'tick ' + name);
  if (!(await run(`F.${side}.sel.has(${JSON.stringify(name)})`))) await click(row(side, name), 'tick ' + name + ' again');
};

try {
  console.log('1. the panel, opened from the tab bar');
  await until(`[...document.querySelectorAll('*')].some(e => e.children.length === 0 && e.textContent.trim() === 'deploy')`, 'the tab bar');
  await click(`() => [...document.querySelectorAll('*')].filter(e => e.children.length === 0 && e.textContent.trim() === 'deploy' && e.getBoundingClientRect().left < 300).pop()`, 'the deploy tab');
  await until(`!document.getElementById('sftppanel').hidden && F.server !== '' && F.remote.rows.length > 0 && F.local.rows.length > 0`, 'both sides listed');
  const server = await run('F.server');
  check(server.includes('127.0.0.1'), 'the connection bar names the server: ' + server);
  shot('1-panel');

  console.log('2. compare');
  await click(more('local', 'notes.txt'), 'notes.txt ...');
  await click(menuItem('sftp.diff'), 'compare');
  await until(`!document.getElementById('sdiff').hidden && !!document.querySelector('#sdiff .dbody .dlines')`, 'the difference');
  const diff = await run(`document.querySelector('#sdiff .dbody').textContent`);
  check(diff.includes('-two') && diff.includes('+TWO'), 'the server\'s line is -two and this PC\'s is +TWO');
  check((await run(`document.querySelector('#sdiff .bwhere').textContent`)).includes(server), 'both copies are named with their machine');
  shot('2-compare');
  await click(`() => document.querySelector('#sdiff .brow .quiet')`, 'close');

  console.log('3. a file the server has never had');
  check(onServer('new.txt') === null, 'new.txt is not on the server before');
  await tick('local', 'new.txt');
  await click(sendButton('local'), 'send');
  check(await waitServer('new.txt', 'made on this PC\n'), 'new.txt arrived on the server, byte for byte');
  await until(`F.queue.length === 0 && F.moving === null`, 'the send to settle');
  await sleep(1500);
  check(await run(`F.said === T['sftp.moved.one'].replace('{name}', 'new.txt')`), 'the panel still says it is done after both lists read again');
  shot('3-sent');

  console.log('4. replacing, with the list first');
  await run(`F.local.sel.clear(); drawSftp(); true`);
  await tick('local', 'notes.txt');
  await click(sendButton('local'), 'send');
  await until(`!document.getElementById('sask').hidden`, 'the replace question');
  check((await run(`document.querySelector('#sask .bwhere').textContent`)).includes(server), 'the question names the machine');
  check((await run(`document.querySelector('#sask .blist').textContent`)).includes(await run(`T['sftp.plan.replace']`)), 'the list marks notes.txt as a replacement');
  shot('4-replace');
  await click(`() => document.querySelector('#sask .go')`, 'replace');
  check(await waitServer('notes.txt', 'one\nTWO\nthree\n'), 'notes.txt on the server is now this PC\'s');
  await until(`F.queue.length === 0 && F.moving === null`, 'the send to settle');

  console.log('5. the delete question names the machine, and is cancelled');
  await until(`F.remote.rows.some(r => r.name === 'new.txt')`, 'new.txt listed on the server side');
  await click(more('remote', 'new.txt'), 'new.txt ...');
  await click(menuItem('sftp.remove'), 'delete');
  await until(`!document.getElementById('sask').hidden`, 'the delete question');
  const del = await run(`document.querySelector('#sask .bwhere').textContent`);
  check(del.includes(server) && del.includes('new.txt'), 'the question names machine and file');
  shot('5-delete');
  await click(`() => document.querySelector('#sask .quiet')`, 'cancel');
  await sleep(800);
  check(onServer('new.txt') !== null, 'cancelled, and new.txt is still there');

  console.log('6. a folder out');
  await run(`F.local.sel.clear(); drawSftp(); true`);
  await click(more('local', 'dist'), 'dist ...');
  await click(menuItem('sftp.send'), 'send');
  await until(`!document.getElementById('sask').hidden`, 'the folder question');
  check(await run(`document.querySelector('#sask .vtitle').textContent === T['sftp.folders.title']`), 'a folder is asked about as a folder');
  shot('6-folder-question');
  await click(`() => document.querySelector('#sask .go')`, 'send the folder');
  check(await waitServer('dist/a/b/deep.txt', 'deep', 60000), 'dist/a/b/deep.txt arrived on the server');
  check(await waitServer('dist/top.txt', 'top'), 'dist/top.txt arrived on the server');
  await until(`F.said === T['sftp.folders.done']`, 'the folder to be reported done', 30000);
  await sleep(1500);
  check(await run(`F.said === T['sftp.folders.done'] && !F.bad`), 'the panel still says the folder is done');
  shot('7-folder-sent');

  console.log('7. a folder back');
  const hero = path.join(WORK, 'assets', 'img', 'hero.png');
  check(!fs.existsSync(hero), 'assets is not on this PC before');
  await until(`F.remote.rows.some(r => r.name === 'assets' && r.dir)`, 'assets listed on the server side');
  await click(more('remote', 'assets'), 'assets ...');
  await click(menuItem('sftp.fetch'), 'bring here');
  await until(`!document.getElementById('sask').hidden`, 'the folder question');
  await click(`() => document.querySelector('#sask .go')`, 'bring the folder');
  const back = Date.now() + 60000;
  while (!fs.existsSync(hero) && Date.now() < back) await sleep(300);
  check(fs.existsSync(hero) && fs.readFileSync(hero, 'utf8') === 'hero-on-the-server', 'assets/img/hero.png arrived on this PC, byte for byte');

  console.log('8. a folder out, with automation stopped');
  await click(`() => [...document.querySelectorAll('button, span, div')].filter(e => e.offsetParent && e.children.length === 0 && e.textContent.trim() === T['tui.stop']).pop()`, 'stop');
  await sleep(800);
  put('later/x.txt', 'later');
  await run(`sftpRefresh(); true`);
  await until(`F.local.rows.some(r => r.name === 'later')`, 'the new folder listed');
  await click(more('local', 'later'), 'later ...');
  await click(menuItem('sftp.send'), 'send');
  await until(`!document.getElementById('sask').hidden`, 'the folder question');
  await click(`() => document.querySelector('#sask .go')`, 'send the folder');
  check(await waitServer('later/x.txt', 'later'), 'the folder went, with automation stopped');
  shot('8-automation-stopped');
} catch (e) {
  check(false, e.message);
} finally {
  ws.close();
  stopApp();
  wsl('bash', '-c', `kill $(cat ${said.serve.replace(/\/serve$/, '')}/etc/sshd.pid) 2>/dev/null`);
}
console.log(failures ? `\n${failures} check(s) failed` : '\nall checks passed');
console.log('photographs: ' + path.relative(ROOT, SHOTS) + '\\sftp-panel-*.png');
process.exit(failures ? 1 : 0);
