/**
 * Is the board's add-a-tab dialog a dialog, on every screen that opens the
 * board in a browser?
 *
 *     node tools/debug/board-dialog.mjs --at <a running copy's folder>
 *
 * Needs Chrome and a copy of the app already running out of that folder with
 * the relay switched on (`tools/debug/instance.win.ps1` makes one). Nothing
 * here ships and nothing is installed.
 *
 * Why it exists. A browser was taken to mean a phone, so the + left the board
 * for a page of settings that filled the screen. A tablet and a Chromebook
 * have room for the board and the dialog both, and the page the + opens is
 * served by the app itself -- there is no way to photograph it without a copy
 * running, and no way to see the fault except at several widths at once.
 *
 * It opens the board at four sizes, presses the tab bar's +, and writes a
 * picture of each into `target/shots/board-dialog`. It also prints, per size,
 * what the frame measures and whether the board is still drawn behind it, so
 * that a wrong answer is a line of text rather than something to spot by eye.
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
const HERE = path.dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1'));
const OUT = path.resolve(HERE, '..', '..', 'target', 'shots', 'board-dialog');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const settings = JSON.parse(fs.readFileSync(path.join(ROOT, 'config', 'config.json'), 'utf8'));
const port = (settings.remote && settings.remote.port) || 8787;
const token = fs.readFileSync(path.join(ROOT, 'data', 'remote-token'), 'utf8').trim();

// The screens that reach this page. The last one is under the size the window
// stops floating a dialog at, where the frame is meant to take everything
const SIZES = [
  { name: 'chromebook', w: 1366, h: 768, mobile: false },
  { name: 'tablet', w: 834, h: 1112, mobile: true },
  { name: 'phone', w: 412, h: 915, mobile: true },
  { name: 'tiny', w: 320, h: 380, mobile: true },
];

function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  for (const base of [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA]) {
    if (!base) continue;
    const p = path.join(base, 'Google', 'Chrome', 'Application', 'chrome.exe');
    if (fs.existsSync(p)) return p;
  }
  throw new Error('no Chrome found; set CHROME');
}

fs.mkdirSync(OUT, { recursive: true });
const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'board-dialog-'));
const chrome = spawn(findChrome(), [
  '--headless=new',
  '--remote-debugging-port=9334',
  '--user-data-dir=' + profile,
  '--no-first-run',
  'about:blank',
], { stdio: 'ignore' });
// However this ends -- a check that failed, a throw -- the browser goes with
// it. The launcher is one process of several, so the tree is what is stopped:
// a left-behind Chrome holds the port, and the next run talks to a browser it
// did not start and cannot explain
process.on('exit', () => {
  try { execFileSync('taskkill.exe', ['/PID', String(chrome.pid), '/T', '/F'], { stdio: 'ignore' }); } catch {}
});
await sleep(1500);

const { attach } = await import(
  new URL('../../.private/doc/proto/webrtc-http/cdp.mjs', import.meta.url).href);
const cdp = await attach(9334);
await cdp.call('Page.enable');
await cdp.call('Runtime.enable');

const js = async (expr) => {
  const r = await cdp.call('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true });
  if (r.result && r.result.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails));
  return r.result && r.result.result && r.result.result.value;
};
// Said to the page without waiting for it to finish: closing the settings is a
// promise that only settles once the question it asks has been answered, and
// answering it is the next thing this does
const fire = (expr) => cdp.call('Runtime.evaluate', { expression: expr, awaitPromise: false });
// Wait for something to become true on the page rather than for a length of
// time: a board that is slow to arrive should not read as a board with no +
async function until(expr, what, tries = 60) {
  for (let i = 0; i < tries; i++) {
    if (await js(expr)) return;
    await sleep(250);
  }
  throw new Error('never happened: ' + what);
}

let bad = 0;
for (const s of SIZES) {
  await cdp.call('Emulation.setDeviceMetricsOverride',
    { width: s.w, height: s.h, deviceScaleFactor: 1, mobile: s.mobile });
  await cdp.call('Page.navigate', { url: `http://127.0.0.1:${port}/?t=${encodeURIComponent(token)}` });
  await until('!!document.querySelector("#strip .snew, .tab.fnew")', 'the + never appeared');
  // Pressed, not called: the road from the button is the thing under test
  await js('(document.querySelector("#strip .snew") || document.querySelector(".tab.fnew")).click()');
  await until('!!document.getElementById("cfglayer")', 'the dialog never opened');
  // And the page inside it is really the dialog, not a page still loading
  await until(
    'document.getElementById("cfglayer").contentDocument && ' +
    'document.getElementById("cfglayer").contentDocument.body && ' +
    'document.getElementById("cfglayer").contentDocument.body.classList.contains("float")',
    'the framed page never became the dialog');
  await sleep(600);

  const m = await js(`(() => {
    const f = document.getElementById("cfglayer");
    // The box, not the frame inside it: the box is what wears the dialog's
    // edge, so it is the rectangle to compare with the window's
    const b = f.closest(".cfgbox").getBoundingClientRect();
    const board = document.getElementById("tabs");
    const seen = board && board.getBoundingClientRect().width > 0 &&
      getComputedStyle(board).visibility !== "hidden";
    return {w: Math.round(b.width), h: Math.round(b.height), x: Math.round(b.left), y: Math.round(b.top),
            full: b.width >= innerWidth && b.height >= innerHeight, board: !!seen,
            title: (f.contentDocument.getElementById("floattitle") || {}).textContent || ""};
  })()`);
  // What the window would do at this size (runtime::dialog_rect), worked out
  // here so the answer is compared with a rule rather than with a picture
  const WIDE = 560, TALL = 640, TOP = 56, EDGE = 16;
  const floats = s.w >= WIDE / 2 + EDGE * 2 && s.h >= TALL / 2 + TOP + EDGE;
  const want = floats
    ? { w: Math.min(WIDE, s.w - EDGE * 2), h: Math.min(TALL, s.h - TOP - EDGE), y: TOP }
    : { w: s.w, h: s.h, y: 0 };
  const ok = m.w === want.w && m.h === want.h && m.y === want.y && (!floats || m.board);
  if (!ok) bad++;
  console.log(
    `${ok ? 'ok  ' : 'BAD '} ${s.name.padEnd(10)} ${s.w}x${s.h}` +
    ` frame ${m.w}x${m.h} at ${m.x},${m.y} (want ${want.w}x${want.h} at y ${want.y})` +
    ` board ${m.board ? 'drawn' : 'gone'} title "${m.title}"`);

  const shot = await cdp.call('Page.captureScreenshot', { format: 'png' });
  fs.writeFileSync(path.join(OUT, `${s.name}.png`), Buffer.from(shot.result.data, 'base64'));
}

// The last screen is the smallest, and the rest of this is about the way out,
// so it is asked at a size with a board around it to come back to
const ROOMY = SIZES[0];
async function openDialog() {
  await cdp.call('Emulation.setDeviceMetricsOverride',
    { width: ROOMY.w, height: ROOMY.h, deviceScaleFactor: 1, mobile: false });
  await cdp.call('Page.navigate', { url: `http://127.0.0.1:${port}/?t=${encodeURIComponent(token)}` });
  await until('!!document.querySelector("#strip .snew, .tab.fnew")', 'the + never appeared');
  await js('(document.querySelector("#strip .snew") || document.querySelector(".tab.fnew")).click()');
  await until('!!document.getElementById("cfglayer") && ' +
    'document.getElementById("cfglayer").contentDocument.body.classList.contains("float")',
    'the dialog never opened');
}
// A keyboard types into the dialog, not into the terminal underneath. With
// real keys about, the board takes the caret for the pane -- so a form framed
// over it has to be the one exception, and the press that proves it is a real
// press, through the browser, not a value set from here
await openDialog();
{
  const where = await js(`(() => {
    const f = document.getElementById("cfglayer");
    const b = f.getBoundingClientRect();
    const i = f.contentDocument.querySelector("#floatbody input.mono");
    const r = i.getBoundingClientRect();
    i.value = "";
    return {x: Math.round(b.left + r.left + r.width / 2), y: Math.round(b.top + r.top + r.height / 2)};
  })()`);
  for (const type of ['mousePressed', 'mouseReleased']) {
    await cdp.call('Input.dispatchMouseEvent',
      { type, x: where.x, y: where.y, button: 'left', clickCount: 1 });
  }
  // Away and back, the way somebody reads something in another tab and returns:
  // that is when the board used to take the caret back
  await cdp.call('Page.bringToFront');
  for (const ch of 'echo hi') {
    await cdp.call('Input.dispatchKeyEvent', { type: 'keyDown', text: ch, key: ch });
    await cdp.call('Input.dispatchKeyEvent', { type: 'keyUp', key: ch });
  }
  await sleep(300);
  const typed = await js(`document.getElementById("cfglayer").contentDocument
    .querySelector("#floatbody input.mono").value`);
  if (typed !== 'echo hi') bad++;
  console.log(`${typed === 'echo hi' ? 'ok  ' : 'BAD '} keys       the field holds "${typed}" (want "echo hi")`);
}
await js(`[...document.getElementById("cfglayer").contentDocument.querySelectorAll("#floatbox .ffoot button")]
  .find(b => b.getAttribute("onclick") === "floatCancel()").click()`);
await until('!document.getElementById("cfglayer")', 'the dialog never closed');

// The thing itself: the tab is added from the frame, the frame goes, and the
// tab is on the board a moment later -- the board never reloaded, so it is the
// one that was there before
await openDialog();
const was = await js('document.querySelectorAll("#strip .stab").length');
await js(`document.getElementById("cfglayer").contentDocument.getElementById("floatadd").click()`);
await until('!document.getElementById("cfglayer")', 'the dialog never closed after adding');
await until(`document.querySelectorAll("#strip .stab").length === ${was + 1}`,
  'the tab never turned up on the board');
console.log(`ok   add        ${was} tab(s) on the board, then ${was + 1}, with no page loaded again`);

// Not adding after all: the frame goes and the board is there, still running --
// the tab bar it was opened from is drawn and no page had to be loaded again
await openDialog();
await js(`[...document.getElementById("cfglayer").contentDocument.querySelectorAll("#floatbox .ffoot button")]
  .find(b => b.getAttribute("onclick") === "floatCancel()").click()`);
await until('!document.getElementById("cfglayer")', 'the dialog never closed');
const back = await js('!!document.querySelector("#strip .snew, .tab.fnew")');
if (!back) bad++;
console.log(`${back ? 'ok  ' : 'BAD '} cancel     the frame goes and the board is still the board`);

// "More settings" is the whole of the settings, so the frame is given the whole
// screen -- and keeps the page it has, with what was chosen so far still chosen
await openDialog();
await js(`[...document.getElementById("cfglayer").contentDocument.querySelectorAll("#floatbox .ffoot button")]
  .find(b => b.getAttribute("onclick") === "floatMore()").click()`);
await until(
  'document.getElementById("cfgwrap").classList.contains("full") && ' +
  '!document.getElementById("cfglayer").contentDocument.body.classList.contains("float")',
  'More settings never took the whole screen');
const more = await js(`(() => {
  const f = document.getElementById("cfglayer"), b = f.closest(".cfgbox").getBoundingClientRect();
  const d = f.contentDocument;
  return {w: Math.round(b.width), h: Math.round(b.height),
          // The whole page: its header and the list of settings beside it
          head: !!d.querySelector("header") && !!d.querySelector("#nav .navitem"),
          // ...still on the tab that was being added
          at: (d.querySelector("#crumb") || {}).textContent || ""};
})()`);
const whole = more.w === ROOMY.w && more.h === ROOMY.h;
if (!whole || !more.head) bad++;
console.log(
  `${whole && more.head ? 'ok  ' : 'BAD '} more       frame ${more.w}x${more.h}` +
  ` (want the whole screen ${ROOMY.w}x${ROOMY.h}) settings header ${more.head ? 'drawn' : 'missing'}` +
  ` at "${more.at.trim()}"`);
{
  const shot = await cdp.call('Page.captureScreenshot', { format: 'png' });
  fs.writeFileSync(path.join(OUT, 'more.png'), Buffer.from(shot.result.data, 'base64'));
}

// Closing the whole of the settings with a tab half added asks first, the way
// it does in the window -- and once answered, the frame goes and the board is
// back with nothing loaded again
await fire('document.getElementById("cfglayer").contentWindow.closeSettings()');
await until('!!document.getElementById("cfglayer").contentDocument.querySelector("dialog.confirm-box")',
  'closing with unsaved changes never asked');
await js(`document.getElementById("cfglayer").contentDocument
  .querySelector("dialog.confirm-box .mfoot button.danger").click()`);
await until('!document.getElementById("cfglayer")', 'the frame never went');
const home = await js('!!document.querySelector("#strip .snew, .tab.fnew")');
if (!home) bad++;
console.log(`${home ? 'ok  ' : 'BAD '} close      it asks about the half-added tab, then gives the board back`);

console.log('pictures in ' + OUT);
cdp.close();
process.exit(bad ? 1 : 0);
