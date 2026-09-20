/**
 * Does a key that types nothing, pressed in the empty sub-input bar, reach the
 * program in the pane -- at the window AND from a browser?
 *
 *     node tools/debug/composer-keys.mjs --at <a running copy's folder>
 *
 * Needs Chrome and a copy of the app already running out of that folder, with
 * the relay switched on and its window's DevTools port open (both are what
 * `tools/debug/instance.win.ps1` sets up). Nothing here ships.
 *
 * Why it exists. The bar was written for a phone, where every key is a soft one
 * and the only thing to do with one is type. Esc, Tab and the arrows therefore
 * stopped in the box -- and Esc is how a person stops an AI. Whether a key now
 * arrives at the other end cannot be answered by reading the page: the proof is
 * the program's own behaviour, so this asks cmd.exe, which answers visibly.
 *
 * It types `echo hello` into the bar and sends it, then, with the box empty,
 * presses the up arrow (the shell puts that line back on its prompt) and Esc
 * (the shell clears the line again). Both are read off the terminal the board
 * draws, on the window's own page and on a browser over the relay.
 */
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const argAt = process.argv.indexOf('--at');
if (argAt < 0) {
  console.error('say which running copy: --at <folder>');
  process.exit(2);
}
const ROOT = path.resolve(process.argv[argAt + 1]);
const settings = JSON.parse(fs.readFileSync(path.join(ROOT, 'config', 'config.json'), 'utf8'));
const port = (settings.remote && settings.remote.port) || 8787;
const token = fs.readFileSync(path.join(ROOT, 'data', 'remote-token'), 'utf8').trim();
// The window's own DevTools port is the board's port plus one (instance.win.ps1)
const winCdp = Number(process.env.WIN_CDP || port + 1);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  for (const base of [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA]) {
    if (!base) continue;
    const p = path.join(base, 'Google', 'Chrome', 'Application', 'chrome.exe');
    if (fs.existsSync(p)) return p;
  }
  throw new Error('no Chrome found; set CHROME');
}

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'composer-keys-'));
const chrome = spawn(findChrome(), [
  '--headless=new',
  '--remote-debugging-port=9338',
  '--user-data-dir=' + profile,
  '--no-first-run',
  'about:blank',
], { stdio: 'ignore' });
process.on('exit', () => {
  try { execFileSync('taskkill.exe', ['/PID', String(chrome.pid), '/T', '/F'], { stdio: 'ignore' }); } catch {}
});
await sleep(1500);

const { attach } = await import(
  new URL('../../.private/doc/proto/webrtc-http/cdp.mjs', import.meta.url).href);

// One surface: a page showing the board, and the keys pressed on it
function surface(cdp) {
  const js = async (expr) => {
    const r = await cdp.call('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true });
    if (r.result && r.result.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails));
    return r.result && r.result.result && r.result.result.value;
  };
  const until = async (expr, what, tries = 60) => {
    for (let i = 0; i < tries; i++) {
      if (await js(expr)) return;
      await sleep(250);
    }
    throw new Error('never happened: ' + what);
  };
  // Pressed, not called: a key dispatched at the page goes wherever the page
  // sends it, which is the whole question here
  const press = async (key, code, keyCode) => {
    for (const type of ['keyDown', 'keyUp']) {
      await cdp.call('Input.dispatchKeyEvent',
        { type, key, code, windowsVirtualKeyCode: keyCode, nativeVirtualKeyCode: keyCode });
    }
    await sleep(500);
  };
  const type = async (text) => {
    for (const ch of text) {
      await cdp.call('Input.dispatchKeyEvent', { type: 'keyDown', text: ch, key: ch });
      await cdp.call('Input.dispatchKeyEvent', { type: 'keyUp', key: ch });
    }
    await sleep(200);
  };
  // What the terminal shows, as the board draws it. Its rows are padded out to
  // the width of the screen, so each line is read without its trailing space
  // and the empty ones are left out
  const lines = async () => {
    const text = await js('document.getElementById("screen").innerText');
    return String(text || '').split('\n').map((l) => l.replace(/\s+$/, '')).filter((l) => l !== '');
  };
  const last = async () => (await lines()).pop() || '';
  // Wait for the pane itself to say something, rather than for the page
  const untilLine = async (want, what, tries = 60) => {
    for (let i = 0; i < tries; i++) {
      if ((await lines()).some(want)) return;
      await sleep(250);
    }
    throw new Error('never happened: ' + what);
  };
  return { js, until, untilLine, press, type, lines, last };
}

// The bar, open and focused, with nothing written in it
async function emptyBar(s) {
  await s.until('typeof openTermBar === "function" && !!document.getElementById("screen") && !!S',
    'the board never arrived');
  // The shell tab, whatever else the copy has been used for since: another
  // tool may have left an AI tab in view, and an AI is not what answers here
  await s.js(`(() => {
    const t = (S.tabs || []).find(x => x.kind === "pty" && !x.settings);
    if (t && S.active !== t.index) send({kind:"select", tab:t.index});
  })()`);
  await s.until('(() => { const t = (S.tabs || []).find(x => x.kind === "pty" && !x.settings);' +
    ' return !!t && S.active === t.index; })()', 'the shell tab never came into view');
  await s.js('rememberCastClosed(false); openTermBar(); castInput.value = ""; growCastInput(); castInput.focus();');
  await s.until('document.activeElement === castInput', 'the caret never reached the input bar');
}

let bad = 0;
async function check(name, cdp) {
  const s = surface(cdp);
  const said = 'hello-' + name;
  await emptyBar(s);
  // A line first, so the shell has something to put back
  await s.type('echo ' + said);
  await s.press('Enter', 'Enter', 13);
  await s.untilLine((l) => l === said, 'the line never ran in the pane');

  // The up arrow with the box empty: the shell puts the line back on its prompt
  await emptyBar(s);
  await s.press('ArrowUp', 'ArrowUp', 38);
  const afterUp = await s.last();
  const recalled = afterUp.endsWith('echo ' + said);
  if (!recalled) bad++;
  console.log(`${recalled ? 'ok  ' : 'BAD '} ${name.padEnd(7)} up    prompt reads "${afterUp}"`);

  // ...and Esc, the key a person stops an AI with: the shell clears the line
  await s.press('Escape', 'Escape', 27);
  const afterEsc = await s.last();
  const cleared = !afterEsc.includes('echo ' + said);
  if (!cleared) bad++;
  console.log(`${cleared ? 'ok  ' : 'BAD '} ${name.padEnd(7)} esc   prompt reads "${afterEsc}"`);

  // The box itself never took any of it
  const held = await s.js('castInput.value');
  if (held !== '') bad++;
  console.log(`${held === '' ? 'ok  ' : 'BAD '} ${name.padEnd(7)} box   holds "${held}" (want nothing)`);
}

// The window: the page the app itself shows
const win = await attach(winCdp);
await win.call('Runtime.enable');
await check('window', win);
win.close();

// And a browser somewhere else, over the relay
const phone = await attach(9338);
await phone.call('Page.enable');
await phone.call('Runtime.enable');
await phone.call('Emulation.setDeviceMetricsOverride',
  { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false });
await phone.call('Page.navigate', { url: `http://127.0.0.1:${port}/?t=${encodeURIComponent(token)}` });
await check('browser', phone);
phone.close();

process.exit(bad ? 1 : 0);
