/**
 * The ideas window, checked through the running app's own window.
 *
 * The page has tests and ideas.rs has tests; what neither can see is the join
 * between them -- the press reaching the app, the app writing the file, and the
 * answer finding its way back into the page while somebody is still typing.
 *
 * So this starts the app built in this checkout, in a folder of its own, with a
 * desk of two git repositories -- one with a worktree cut somewhere else
 * entirely -- and a folder that is no repository, and uses the ideas the way a
 * person does: the
 * bulb, typing, Enter, Shift+Enter, ticking one off, carrying one by its grip,
 * a card with no project, and a project taken out of the settings. Every step
 * is checked against config/ideas.json itself, not only against the window.
 *
 *     cargo build
 *     node tools/debug/ideas.win.mjs
 *
 * Needs Windows and Node. Isolated the way a new-user run is: its own folder,
 * its own LOCALAPPDATA (the WebView2 store lives there), and started from its
 * own folder. Nothing of a copy somebody is using is read, written or stopped.
 * Photographs land in target/shots; the app is stopped on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const PORT = 9342;
// Short, under the temporary folder: the WebView2 store nests deeply, and a
// long base path runs past what Windows allows
const RUN = path.join(os.tmpdir(), 'sk-ideas');
const APP = path.join(RUN, 'app');
const APPDIR = path.join(RUN, 'work', 'app');
const NOTES = path.join(RUN, 'work', 'notes');
// A worktree of app, away from it: the same project however far away it is
const BRANCH = path.join(RUN, 'elsewhere', 'fix');
// No repository at all: no project
const PLAIN = path.join(RUN, 'work', 'plain');
const FILE = path.join(APP, 'config', 'ideas.json');
const CONFIG = path.join(APP, 'config', 'config.json');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });

/** This checkout's copy, stopped by where it runs from and nothing else */
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
for (const d of [APP, APPDIR, NOTES, PLAIN, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });
const git = (cwd, ...args) => {
  const r = spawnSync('git', ['-c', 'user.name=check', '-c', 'user.email=check@example.com', ...args], { cwd, encoding: 'utf8' });
  if (r.status !== 0) die('git ' + args.join(' ') + ' failed:\n' + r.stderr);
};
for (const repo of [APPDIR, NOTES]) {
  git(repo, 'init', '-q');
  git(repo, 'commit', '-q', '--allow-empty', '-m', 'start');
}
fs.mkdirSync(path.dirname(BRANCH), { recursive: true });
git(APPDIR, 'worktree', 'add', '-q', '-b', 'fix', BRANCH);
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);

const settings = (folders) => ({
  language: 'ja',
  remote: { enabled: false },
  desks: [{ name: 'Check', id: 'check', folders }],
});
const appFolder = { cwd: APPDIR, tabs: [{ name: 'app-shell', id: 'app-shell', command: 'cmd.exe' }] };
const notesFolder = { cwd: NOTES, tabs: [{ name: 'notes-shell', id: 'notes-shell', command: 'cmd.exe' }] };
const branchFolder = { cwd: BRANCH, tabs: [{ name: 'fix-shell', id: 'fix-shell', command: 'cmd.exe' }] };
const plainFolder = { cwd: PLAIN, tabs: [{ name: 'plain-shell', id: 'plain-shell', command: 'cmd.exe' }] };
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify(settings([appFolder, branchFolder, notesFolder, plainFolder]), null, 2));

// Started directly, so the isolated LOCALAPPDATA and the DevTools port reach
// it. What says "you are inside Claude Code" is taken out, so the app is not
// told it is somebody's child session
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${PORT}`;
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

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
const until = async (test, what, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test().catch(() => false)) return true; await sleep(150); }
  throw new Error('timed out waiting for ' + what);
};
const at = async (sel) => run(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return null;
  const r = e.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
const click = async (sel) => {
  const p = await at(sel);
  if (!p) throw new Error('nothing to click: ' + sel);
  for (const type of ['mouseMoved', 'mousePressed', 'mouseReleased']) {
    await send('Input.dispatchMouseEvent', { type, x: p.x, y: p.y, button: 'left', clickCount: 1 });
  }
  await sleep(200);
};
const type = (text) => send('Input.insertText', { text });
const key = async (name, modifiers = 0) => {
  const code = { Enter: 13, Escape: 27, End: 35 }[name];
  const base = { key: name, code: name, windowsVirtualKeyCode: code, modifiers };
  await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', ...base });
  if (name === 'Enter') await send('Input.dispatchKeyEvent', { type: 'char', ...base, text: '\r' });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
};
const onDisk = () => { try { return JSON.parse(fs.readFileSync(FILE, 'utf8')).items; } catch { return []; } };
const disk = (test, what, ms) => until(async () => test(onDisk()), what, ms);
const shot = (name) => ps('-File', path.join(ROOT, 'tools', 'debug', 'shot-window.win.ps1'),
  '-Under', RUN, '-Out', path.join(SHOTS, `ideas-${name}.png`));
const same = (a, b) => (a || '').replace(/[\\/]+$/, '').toLowerCase() === (b || '').replace(/[\\/]+$/, '').toLowerCase();

try {
  console.log('1. the bulb opens the ideas, the caret in the writing line');
  await until(() => run(`!!document.querySelector('.gearrow .ideabtn') && !!(S && S.groups && S.groups.length === 4)`), 'the side column');
  await click('.gearrow .ideabtn');
  await until(() => run(`!document.getElementById('ideas').hidden && IDEAS.known`), 'the ideas and their projects');
  check(await run(`document.activeElement.matches('#ideas .inew .itext')`), 'the caret is in the writing line');
  const chosen = await run(`document.querySelector('#ideas .iproj').value`);
  check(same(chosen, APPDIR), 'the project of the folder in front is chosen: ' + chosen);
  const offered = await run(`IDEAS.projects.map(p => p.name + ':' + p.folders.length).sort().join(',')`);
  check(offered === 'app:2,notes:1', "the two repositories are offered, the worktree elsewhere counted as app's, the plain folder not at all: " + offered);

  console.log('2. writing cards');
  await type('最初のアイデア'); await key('Enter');
  await type('二つ目'); await key('Enter', 8); await type('二行目'); await key('Enter');
  await disk((items) => items.length === 2 && items[1].text === '二つ目\n二行目', 'two cards in the file');
  const items = onDisk();
  check(items[0].text === '最初のアイデア' && same(items[0].project, APPDIR), 'the first card is saved, in its project');
  check(items[1].text === '二つ目\n二行目', 'Shift+Enter kept a new line inside the card');
  check(await run(`document.querySelectorAll('#ideas .ilist .icard').length === 2`), 'both are drawn');
  shot('1-written');

  console.log('3. an edit is saved with no button');
  await click('#ideas .ilist .icard:nth-child(1) .itext');
  await key('End'); await type('（直した）');
  await disk((it) => it[0] && it[0].text === '最初のアイデア（直した）', 'the edit in the file');
  check(true, 'the edit reached the file');

  console.log('4. ticked off');
  await click('#ideas .ilist .icard:nth-child(1) .icheck');
  await disk((it) => it[0] && it[0].done === true, 'done in the file');
  check(await run(`document.querySelectorAll('#ideas .ilist .icard').length === 1`), 'it left the list');
  await click('#ideas .idone');
  check(await run(`document.querySelectorAll('#ideas .ilist .icard.done').length === 1`), 'shown again, marked done, when done ones are shown');
  await click('#ideas .idone');

  console.log('5. carried by its grip');
  await type('三つ目'); await key('Enter');
  await disk((it) => it.length === 3, 'the third card');
  await sleep(300);
  const from = await at('#ideas .ilist .icard:nth-child(2) .igrip');
  const to = await at('#ideas .ilist .icard:nth-child(1) .igrip');
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', x: from.x, y: from.y, button: 'left', clickCount: 1 });
  for (let k = 1; k <= 8; k++) {
    await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: from.x, y: from.y + (to.y - 12 - from.y) * k / 8, button: 'left', buttons: 1 });
  }
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: to.x, y: to.y - 12, button: 'left', clickCount: 1 });
  await disk((it) => it.map((i) => i.text).join('|') === '最初のアイデア（直した）|三つ目|二つ目\n二行目', 'the new order in the file');
  check(true, 'the order reached the file, the done card keeping its place');
  shot('2-reordered');

  console.log('6. a card with no project, and one for the other project');
  await run(`(() => { const p = document.querySelector('#ideas .iproj'); p.value = ''; p.dispatchEvent(new Event('change')); })()`);
  await type('どこにも属さないメモ'); await key('Enter');
  const notesKey = await run(`IDEAS.projects.find(p => p.name === 'notes').key`);
  await run(`(() => { const p = document.querySelector('#ideas .iproj'); p.value = ${JSON.stringify(notesKey)}; p.dispatchEvent(new Event('change')); })()`);
  await type('notes のメモ'); await key('Enter');
  await disk((it) => it.some((i) => i.text === 'notes のメモ'), 'the notes card');
  check(onDisk().some((i) => i.text === 'どこにも属さないメモ' && i.project === null), 'the card with no project has none');
  check(same(onDisk().find((i) => i.text === 'notes のメモ').project, NOTES), 'the notes card belongs to notes');

  console.log('6a. the right-click menu: done keeps a card, delete takes it out of the file');
  await type('消すメモ'); await key('Enter');
  await type('しまうメモ'); await key('Enter');
  await disk((it) => it.some((i) => i.text === 'しまうメモ'), 'two more notes cards');
  await sleep(300);
  const rightClick = async (text) => {
    const p = await run(`(() => { const c = [...document.querySelectorAll('#ideas .ilist .icard')].find(c => c.querySelector('.itext').value === ${JSON.stringify(text)});
      const r = c.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
    await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: p.x, y: p.y });
    await send('Input.dispatchMouseEvent', { type: 'mousePressed', x: p.x, y: p.y, button: 'right', clickCount: 1 });
    await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: p.x, y: p.y, button: 'right', clickCount: 1 });
    await until(() => run(`!!document.querySelector('.fmenu')`), 'the menu');
  };
  const pick = async (key) => {
    const p = await run(`(() => { const d = [...document.querySelectorAll('.fmenu div')].find(d => d.textContent === T[${JSON.stringify(key)}]);
      const r = d.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
    for (const type of ['mouseMoved', 'mousePressed', 'mouseReleased']) {
      await send('Input.dispatchMouseEvent', { type, x: p.x, y: p.y, button: 'left', clickCount: 1 });
    }
  };
  await rightClick('消すメモ');
  check((await run(`[...document.querySelectorAll('.fmenu div')].map(d => d.textContent).join('|')`)) === 'コピー|Issue に送る|完了にする|削除', 'the menu says copy, send to Issue, done and delete, in that order');
  check(await run(`document.querySelector('.fmenu div:last-child').classList.contains('warn')`), 'delete is the red last line');
  await pick('tui.ideas.delete');
  await disk((it) => !it.some((i) => i.text === '消すメモ'), 'the deleted card gone from the file');
  check(!(await run(`[...document.querySelectorAll('#ideas .icard .itext')].some(t => t.value === '消すメモ')`)), 'deleted, it left the list');
  await rightClick('しまうメモ');
  await pick('tui.ideas.markdone');
  await disk((it) => it.some((i) => i.text === 'しまうメモ' && i.done), 'done in the file');
  await click('#ideas .idone');
  check(await run(`[...document.querySelectorAll('#ideas .icard.done .itext')].some(t => t.value === 'しまうメモ')`), 'done, it comes back with the done ones');
  check(!(await run(`[...document.querySelectorAll('#ideas .icard .itext')].some(t => t.value === '消すメモ')`)), 'deleted, it does not come back with them');
  await rightClick('しまうメモ');
  check((await run(`[...document.querySelectorAll('.fmenu div')].map(d => d.textContent).join('|')`)) === 'コピー|Issue に送る|未完了に戻す|削除', 'a done card offers to be not done');
  await key('Escape');
  check(await run(`!document.querySelector('.fmenu') && !document.getElementById('ideas').hidden`), 'Esc puts the menu away and leaves the ideas up');
  await click('#ideas .idone');

  console.log('6b. Esc closes');
  await key('Escape');
  check(await run(`document.getElementById('ideas').hidden`), 'Esc closed the ideas');

  console.log('7. from the worktree elsewhere, the project is app');
  await run(`send({kind:'select', tab: S.tabs.find(t => t.name === 'fix-shell').index}); true`);
  await until(() => run(`activeTab() && activeTab().name === 'fix-shell'`), 'the worktree tab in front');
  await click('.gearrow .ideabtn');
  await until(() => run(`!document.getElementById('ideas').hidden && IDEAS.known`), 'the ideas');
  check(same(await run(`document.querySelector('#ideas .iproj').value`), APPDIR), "the worktree's folder opens on app");
  check((await run(`document.querySelectorAll('#ideas .ilist .icard').length`)) === 2, "with app's cards");
  await key('Escape');

  console.log('8. the settings lose the notes repository');
  fs.writeFileSync(CONFIG, JSON.stringify(settings([appFolder, branchFolder, plainFolder]), null, 2));
  await until(() => run(`S && S.groups && S.groups.length === 3`), 'the settings to be read again', 30000);
  await click('.gearrow .ideabtn');
  await disk((it) => { const n = it.find((i) => i.text === 'notes のメモ'); return n && n.project === null; }, 'the notes card moved to no project');
  check(true, 'the card of the removed project went to no project');
  check((await run(`IDEAS.projects.length`)) === 1, 'the removed project is no longer offered');
  check(onDisk().some((i) => i.text === '最初のアイデア（直した）' && same(i.project, APPDIR)), "app's cards stayed with app");
  shot('3-project-removed');

  console.log('9. sent to an Issue: the Issue tab comes to the front with the idea as the new issue\'s description');
  await until(() => run(`document.querySelectorAll('#ideas .ilist .icard').length > 0`), 'app\'s cards');
  const sent = await run(`document.querySelector('#ideas .ilist .icard .itext').value`);
  await click('#ideas .ilist .icard .itool:last-child');
  await until(() => run(`document.getElementById('ideas').hidden && !document.getElementById('issuespanel').hidden`), 'the Issue tab in front', 30000);
  check(true, 'the ideas closed and the Issue tab came to the front');
  await until(() => run(`I.projects !== null && I.view === 'create' && !!document.querySelector('#issuespanel textarea')`), 'the new issue form');
  check((await run(`document.querySelector('#issuespanel textarea').value`)) === sent, 'the description holds the idea');
  check((await run(`I.create.project`)) === 'app', 'the idea\'s project is chosen: ' + (await run(`I.create.project`)));
  check(await run(`!!document.querySelector('#issuespanel .iai')`), 'the ✨ that writes the rest from it is there');
  shot('4-sent-to-issue');

  console.log('10. put away, the idea is left as it was; made, it is done and names the issue');
  await click('#issuespanel .foot .quiet');
  await sleep(500);
  check(!onDisk().find((i) => i.text === sent).done, 'cancelling the form leaves the idea not done');
  // Back on the form, GitHub's answer to "make it" is handed to the page as the
  // app hands it, rather than making a real issue on somebody's account
  await run(`(() => { I.view = 'create'; issuesSig = ''; drawIssues(); return true; })()`);
  await run(`window.__issues({act:'create', ok:true, kind:'issue', project:'app', seq:null, data:{number:12, url:'https://github.com/example/app/issues/12'}}); true`);
  await disk((it) => { const c = it.find((i) => i.text === sent); return c && c.done && c.issue && c.issue.number === 12; }, 'the idea done, with issue 12');
  check(true, 'made, the idea is done and remembers issue #12');
  await click('.gearrow .ideabtn');
  await until(() => run(`!document.getElementById('ideas').hidden && IDEAS.known`), 'the ideas');
  // The Issue tab is in front, so no folder is: app is chosen by hand
  const appKey = await run(`IDEAS.projects.find(p => p.name === 'app').key`);
  await run(`(() => { const p = document.querySelector('#ideas .iproj'); p.value = ${JSON.stringify(appKey)}; p.dispatchEvent(new Event('change')); })()`);
  check(!(await run(`[...document.querySelectorAll('#ideas .icard .itext')].some(t => t.value === ${JSON.stringify(sent)})`)), 'done, it is out of the list');
  await click('#ideas .idone');
  const mark = await run(`(() => { const c = [...document.querySelectorAll('#ideas .icard')].find(c => c.querySelector('.itext').value === ${JSON.stringify(sent)}); return c ? c.querySelector('.iissue')?.textContent : null; })()`);
  check(mark === '#12', 'shown with the done ones, it wears #12: ' + mark);
  shot('5-issue-mark');
  await key('Escape');
} catch (e) {
  check(false, e.message);
} finally {
  ws.close();
  stopApp();
}
console.log(failures ? `\n${failures} check(s) failed` : '\nall checks passed');
console.log('photographs: ' + path.relative(ROOT, SHOTS) + '\\ideas-*.png');
process.exit(failures ? 1 : 0);
