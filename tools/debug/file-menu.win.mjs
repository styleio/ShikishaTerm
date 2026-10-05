/**
 * The file list's own menu, checked through the running app's own window, and
 * against the files on the disk afterwards.
 *
 * Renaming, duplicating and deleting each run across the join between the
 * window, the loop and the disk -- and no test stands there. So this
 * right-clicks a real row, reads the real list, answers the real question,
 * and then looks at the folder.
 *
 *     cargo build
 *     node tools/debug/file-menu.win.mjs
 *
 * Needs Windows and Node. The copy it drives is started by `instance.win.ps1`,
 * so it has its own folder, settings, state and ports, and nothing of a copy
 * somebody is using is read, written or stopped. The copy is stopped on the
 * way out. The folder deleted here goes to this PC's recycle bin.
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-filemenu');
// Beside the copy's folder, not inside it: the copy clears its own on the way up
const WORK = RUN + '-work';
const INSTANCE = path.join(ROOT, 'tools', 'debug', 'instance.win.ps1');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => {
  console.log((ok ? '  PASS ' : '  FAIL ') + what);
  if (!ok) failures += 1;
};
const ps = (args) => spawnSync('powershell.exe',
  ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', INSTANCE, ...args],
  { encoding: 'utf8' });
const here = (name) => fs.existsSync(path.join(WORK, name));

// ── A folder with a file and a folder in it ───────────────────────────────
fs.rmSync(RUN, { recursive: true, force: true });
fs.rmSync(WORK, { recursive: true, force: true });
fs.mkdirSync(path.join(WORK, 'sub'), { recursive: true });
fs.writeFileSync(path.join(WORK, 'a.txt'), 'one\n');
fs.writeFileSync(path.join(WORK, 'sub', 'b.txt'), 'two\n');

// ── The copy, and its window ──────────────────────────────────────────────
const started = ps(['-At', RUN, '-Work', WORK]);
if (started.status !== 0) die(started.stdout + started.stderr);
const said = Object.fromEntries((started.stdout || '').split('\n')
  .map((l) => l.trim().split('=')).filter((p) => p.length > 1)
  .map(([k, ...v]) => [k, v.join('=')]));
const cdp = Number((said.cdp || '').split(':').pop());
if (!cdp) die('the copy did not say where its DevTools port is:\n' + started.stdout);
const stop = () => ps(['-At', RUN, '-Stop']);

let ws;
try {
  let targets = [];
  for (let i = 0; i < 80 && !targets.length; i++) {
    try {
      targets = (await (await fetch(`http://127.0.0.1:${cdp}/json/list`)).json())
        .filter((t) => t.type === 'page');
    } catch { await sleep(250); }
  }
  if (!targets.length) throw new Error('the window never answered on its DevTools port');
  const connection = await connectCdp(targets[0]);
  ws = connection.ws;
  const {run} = connection;

  // The column, open on the files, read again from the top
  const open = `(async () => {
    sidePanel = "files"; setSideWidth(440); drawSide();
    await new Promise(r => setTimeout(r, 400));
    FS.panel = null; drawSide();
    await new Promise(r => setTimeout(r, 1200));
    return [...document.querySelectorAll("#filepanel .frow")].map(e => e.title);
  })()`;

  // One row's list: right-click it, and say what the list holds. With `entry`,
  // press that entry; with `field`, type it into the question; with `go`,
  // answer the question with its button
  const menu = (row, entry, field, go) => `(async () => {
    const wait = ms => new Promise(r => setTimeout(r, ms));
    const it = [...document.querySelectorAll("#filepanel .frow")].find(r => r.title === ${JSON.stringify(row)});
    if (!it) return {error: "no row for " + ${JSON.stringify(row)}};
    const box = it.getBoundingClientRect();
    it.dispatchEvent(new MouseEvent("contextmenu", {bubbles:true, cancelable:true,
      clientX: Math.round(box.left + 40), clientY: Math.round(box.top + 6)}));
    await wait(200);
    const items = [...document.querySelectorAll(".fmenu > div")];
    const out = {items: items.map(e => e.textContent), last_red: !!items.length && items[items.length - 1].classList.contains("warn")};
    const want = items.find(e => e.textContent === T[${JSON.stringify(entry)}]);
    if (!want) { closeFolderMenu(); return out; }
    want.click();
    await wait(400);
    const ask = document.getElementById("sask");
    out.asked = !ask.hidden;
    out.say = out.asked ? ask.querySelector(".vsay").textContent : "";
    out.red = out.asked && ask.querySelector(".go").classList.contains("stop");
    if (out.asked) {
      if (${JSON.stringify(field)} !== null) ask.querySelector("#sq").value = ${JSON.stringify(field)};
      if (${go ? 'true' : 'false'}) ask.querySelector(".go").click(); else closeAsk();
    }
    await wait(1500);
    out.after = FS.said || "";
    out.rows = [...document.querySelectorAll("#filepanel .frow")].map(e => e.title);
    return out;
  })()`;

  const listed = await run(open);
  console.log('the list shows: ' + listed.join(', '));
  check(listed.includes('a.txt') && listed.includes('sub'), 'the file and the folder are listed');

  // A file's list: edit, rename, duplicate, and delete last and in red
  let r = await run(menu('a.txt', 'files.menu.rename', 'c.txt', true));
  console.log('  ' + JSON.stringify(r));
  check(r.items && r.items.length === 4, 'a file offers four entries');
  check(r.last_red, 'delete is last, in red');
  check(!here('a.txt') && here('c.txt'), 'the file is renamed on the disk');
  check(r.rows && r.rows.includes('c.txt') && !r.rows.includes('a.txt'), 'the list shows the new name');

  // A name already there is refused, and nothing is written over
  fs.writeFileSync(path.join(WORK, 'd.txt'), 'kept\n');
  r = await run(menu('c.txt', 'files.menu.rename', 'd.txt', true));
  console.log('  ' + JSON.stringify(r));
  check(fs.readFileSync(path.join(WORK, 'd.txt'), 'utf8') === 'kept\n' && here('c.txt'), 'a rename over a name already there is refused');
  check(!!r.after, 'the refusal is said');

  // Duplicated beside itself, twice: the second copy takes the next name
  r = await run(menu('c.txt', 'files.menu.copy', null, false));
  console.log('  ' + JSON.stringify(r));
  const copies = fs.readdirSync(WORK).filter((n) => n.startsWith('c') && n !== 'c.txt');
  check(copies.length === 1 && fs.readFileSync(path.join(WORK, copies[0]), 'utf8') === 'one\n', 'the file is duplicated: ' + copies.join(', '));
  r = await run(menu('c.txt', 'files.menu.copy', null, false));
  const again = fs.readdirSync(WORK).filter((n) => n.startsWith('c') && n !== 'c.txt');
  check(again.length === 2, 'a second copy takes another name: ' + again.join(', '));

  // A folder: duplicated whole, then deleted -- the question asks first
  r = await run(menu('sub', 'files.menu.copy', null, false));
  console.log('  ' + JSON.stringify(r));
  check(r.items && r.items.length === 3, 'a folder offers no "edit"');
  const subs = fs.readdirSync(WORK).filter((n) => n.startsWith('sub') && n !== 'sub');
  check(subs.length === 1 && fs.existsSync(path.join(WORK, subs[0], 'b.txt')), 'the folder is copied whole: ' + subs.join(', '));
  r = await run(menu('sub', 'files.menu.remove', null, false));
  check(r.asked && here('sub'), 'delete asks first, and Cancel leaves it');
  check(r.red, 'the button that deletes is the red one');
  r = await run(menu('sub', 'files.menu.remove', null, true));
  console.log('  ' + JSON.stringify(r));
  check(!here('sub'), 'the folder is gone from the disk');
  check(r.rows && !r.rows.includes('sub'), 'and from the list');
} catch (e) {
  console.error(e.message || e);
  failures += 1;
} finally {
  if (ws) ws.close();
  stop();
  fs.rmSync(WORK, { recursive: true, force: true });
}

console.log(failures ? failures + ' failed' : 'all good');
process.exit(failures ? 1 : 0);
