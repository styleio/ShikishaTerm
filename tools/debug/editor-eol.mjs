/**
 * A file's line endings through the editor, with the real editor library.
 *
 * The page as the app serves it, in a headless Chrome, with its editor opened
 * on a file the way the app opens one (the same answer `files_answer` sends).
 * Text is typed, removed, pasted and undone through the library itself, and
 * what the page would save is read back. The unit test beside the page
 * (the_editor_keeps_each_lines_own_ending) drives the same functions against
 * a stand-in for the library; this is the library.
 *
 *     node tools/debug/editor-eol.mjs
 *
 * Checked:
 *   mixed   a file that ends its lines two ways is not "changed" by being
 *           opened, an edit changes only the lines it touches, lines typed in
 *           take the ending most of the file uses, undo puts endings back
 *   crlf    a CRLF file stays CRLF, typed lines too
 *   lf      an LF file stays LF
 *
 * Needs Node and Chrome. The editor library is copied beside the page.
 */
import {findChrome, findCargo, startChrome} from './chrome.mjs';
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const OUT = path.join(ROOT, 'target', 'shots');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };

fs.mkdirSync(path.join(OUT, 'vendor'), { recursive: true });
fs.cpSync(path.join(ROOT, 'vendor', 'ace'), path.join(OUT, 'vendor', 'ace'), { recursive: true });
const made = spawnSync(findCargo(), ['run', '--quiet', '--bin', 'page_dump', '--', 'en'], { cwd: ROOT, maxBuffer: 1 << 28 });
if (made.status !== 0) { console.error('page_dump failed: ' + made.stderr); process.exit(2); }
const file = path.join(OUT, `page.${process.pid}.eol.html`);
fs.writeFileSync(file, made.stdout);

const state = {
  desk: 'shop', desk_id: 'shop', desks: ['shop'], desk_index: 0, active: 2,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [{ name: 'shop', folder: 'D:/work/shop', color: '#4285f4', linked: false, family: 'D:/work/shop/.git', branch: 'main',
    health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } }],
  tabs: [{ index: 2, name: 'editor', id: 'editor', state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
    locked: false, depth: 0, activity: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0], group: 0, kind: 'editor',
    model: false, busy: false, settings: false, auto: false, restartable: true, readable: false,
    key: 'tab:2', file: 'notes.txt' }],
  making: [],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
};

const chrome = await startChrome({ chrome: findChrome() });
try {
  await chrome.send('Page.enable');
  await chrome.send('Runtime.enable');
  await chrome.send('Page.addScriptToEvaluateOnNewDocument', { source: 'window.ipc = { postMessage(){} };' });
  await chrome.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 860, deviceScaleFactor: 1, mobile: false });
  await chrome.send('Page.navigate', { url: pathToFileURL(file).href });
  await sleep(1200);
  // Open a file with this text, the way the app answers a read
  let n = 0;
  const open = async (text) => {
    n += 1;
    // A file of its own each time: one with typing in it is not read over
    const name = 'notes' + n + '.txt';
    state.tabs[0].file = name;
    const answer = { act: 'read', ok: true, path: name, text, encoding: 'UTF-8', exact: true, mark: 'm' + n, stamp: 's' + n };
    await chrome.run(`window.__state(${JSON.stringify(JSON.stringify(state))}); window.__files(${JSON.stringify(answer)}); "ok"`);
    for (let i = 0; i < 80; i++) {
      if (await chrome.run(`!!edAce && ED.text === ${JSON.stringify(text)} && edAce.session.getLength() > 0`)) return;
      await sleep(100);
    }
    throw new Error('the editor did not open the file');
  };
  const text = () => chrome.run('edText()');
  const dirty = () => chrome.run('ED.dirty');
  const j = JSON.stringify;

  console.log('1. a file that ends its lines two ways');
  const mixed = 'one\r\ntwo\nthree\r\nfour\nfive\n';
  await open(mixed);
  check((await text()) === mixed && !(await dirty()), 'opened, it is the file and is not called changed: ' + j(await text()));
  await chrome.run('edAce.session.insert({row: 1, column: 3}, "!"); "ok"');
  check((await text()) === 'one\r\ntwo!\nthree\r\nfour\nfive\n' && (await dirty()), 'a character typed changes its line and no ending: ' + j(await text()));
  await chrome.run('edAce.session.insert({row: 0, column: 3}, "\\nnew"); "ok"');
  check((await text()) === 'one\nnew\r\ntwo!\nthree\r\nfour\nfive\n', 'a line typed in takes the ending most of the file uses: ' + j(await text()));
  await chrome.run('edAce.session.remove({start: {row: 1, column: 3}, end: {row: 3, column: 0}}); "ok"');
  check((await text()) === 'one\nnewthree\r\nfour\nfive\n', 'lines taken out take their endings with them: ' + j(await text()));
  await chrome.run('edAce.undo(); "ok"');
  check((await text()) === 'one\nnew\r\ntwo!\nthree\r\nfour\nfive\n', 'undo puts the lines back with the endings they had: ' + j(await text()));
  await chrome.run('edAce.undo(); edAce.undo(); "ok"');
  check((await text()) === mixed && !(await dirty()), 'everything undone, it is the file again and not called changed: ' + j(await text()));
  // A paste over a selection, the way the keyboard does it
  await open(mixed);
  await chrome.run('edAce.selection.setRange({start: {row: 2, column: 0}, end: {row: 2, column: 5}}); edAce.onPaste("a\\nb\\nc"); "ok"');
  check((await text()) === 'one\r\ntwo\na\nb\nc\r\nfour\nfive\n', 'a paste of lines keeps the endings around it: ' + j(await text()));
  // Ctrl+A, type: everything replaced
  await chrome.run('edAce.selectAll(); edAce.onPaste("x\\ny"); "ok"');
  check((await text()) === 'x\ny', 'everything replaced is the new text, with the file\'s usual ending: ' + j(await text()));

  console.log('2. a CRLF file');
  const crlf = 'a\r\nb\r\nc\r\n';
  await open(crlf);
  check((await text()) === crlf && !(await dirty()), 'opened, it is the file');
  await chrome.run('edAce.session.insert({row: 0, column: 1}, "\\nx"); "ok"');
  check((await text()) === 'a\r\nx\r\nb\r\nc\r\n', 'a typed line ends with CRLF: ' + j(await text()));
  // A CRLF file whose first line happens to end with LF used to turn LF throughout
  const mostly = 'head\n' + 'row\r\n'.repeat(10);
  await open(mostly);
  check((await text()) === mostly && !(await dirty()), 'a CRLF file with one LF line is not rewritten by being opened');
  await chrome.run('edAce.session.insert({row: 5, column: 0}, "added\\n"); "ok"');
  check((await text()) === 'head\n' + 'row\r\n'.repeat(4) + 'added\r\n' + 'row\r\n'.repeat(6), 'and a line typed into it ends the way most of it does: ' + j((await text()).slice(0, 60)));

  console.log('3. an LF file');
  const lf = 'a\nb\nc\n';
  await open(lf);
  await chrome.run('edAce.session.insert({row: 1, column: 1}, "\\nx"); "ok"');
  check((await text()) === 'a\nb\nx\nc\n', 'stays LF: ' + j(await text()));
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + e.message);
} finally {
  chrome.stop();
  fs.rmSync(file, { force: true });
}
console.log(failures ? `${failures} check(s) failed` : 'all checks passed');
process.exit(failures ? 1 : 0);
