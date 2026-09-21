/**
 * Does the ? open a panel, and does the panel stay inside the window?
 *
 *     node tools/debug/guide-panel.win.mjs --cdp 9401 --pid 23488
 *
 * Needs a copy of the app already running with its window's DevTools port open
 * (`tools/debug/instance.win.ps1` makes one and prints both numbers). Nothing
 * here ships and nothing is installed.
 *
 * Why it exists. The panel is a page the app *places*, so nothing on the board
 * can see it: it is not in the board's DOM, it is not a tab, and a synthetic
 * mouse click on this machine does not reach the board at all. What can be
 * seen is the board's own side of it -- that the button is there, that pressing
 * it is reported, that pressing it again takes it away -- and, in a picture of
 * the window, the panel itself. This does the first by driving the page and
 * leaves the second as `target/shots/guide-panel/*.png` to look at.
 *
 * Its own ✕ and the drag are the panel's, inside a page this cannot reach;
 * check those by hand in the picture and with the mouse.
 */
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const arg = (name, fallback) => {
  const at = process.argv.indexOf('--' + name);
  return at < 0 ? fallback : process.argv[at + 1];
};
const CDP = arg('cdp', '9401');
const PID = arg('pid', '');
const HERE = path.dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1'));
const OUT = path.resolve(HERE, '..', '..', 'target', 'shots', 'guide-panel');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const pages = await (await fetch(`http://127.0.0.1:${CDP}/json/list`)).json();
const board = pages.find((p) => p.type === 'page');
if (!board) throw new Error('nothing is listening on ' + CDP);

const ws = new WebSocket(board.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
const call = (method, params) =>
  new Promise((res) => {
    const id = Math.floor(Math.random() * 1e6);
    const on = (e) => {
      const m = JSON.parse(e.data);
      if (m.id === id) {
        ws.removeEventListener('message', on);
        res(m.result);
      }
    };
    ws.addEventListener('message', on);
    ws.send(JSON.stringify({ id, method, params }));
  });
const js = async (expression) => {
  const r = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails));
  return r.result && r.result.value;
};

fs.mkdirSync(OUT, { recursive: true });
const shoot = (name) => {
  if (!PID) return;
  execFileSync('powershell.exe', [
    '-NoProfile', '-File', path.join(HERE, 'shot-window.win.ps1'),
    '-ProcessId', PID, '-Out', path.join(OUT, name + '.png'),
  ], { stdio: 'ignore' });
};

let bad = 0;
const check = (ok, what) => {
  console.log((ok ? 'ok   ' : 'BAD  ') + what);
  if (!ok) bad++;
};

check(await js('!!document.querySelector(".sidebtn.help")'), 'the ? is on the board');
check(
  await js('!document.querySelector(".sidebtn.help").getAttribute("href")'),
  'the ? no longer leaves for the site',
);

// Whether it was already up is not known from here -- the panel is not in
// this page -- so it is pressed once and photographed, and the picture that
// is empty says which way round the two presses went
await js('document.querySelector(".sidebtn.help").click()');
await sleep(2500);
shoot('one');
console.log('     pictures after each press are in ' + OUT + ' (one.png, two.png):');
console.log('     one of the two has the panel in it and the other has not');

// Pressed again, it goes. Nothing on the board says so, so this is the app's
// own answer: the second press is reported the same way as the first
await js('document.querySelector(".sidebtn.help").click()');
await sleep(2000);
shoot('two');

check(
  await js('!!window.toggleGuide && typeof toggleGuide === "function"'),
  'the board knows how to open and shut it',
);
check(await js('!!document.getElementById("guidewrap")'), "a phone's sheet has somewhere to go");

ws.close();
console.log(bad ? `${bad} wrong` : 'all good; look at the pictures for the panel itself');
process.exit(bad ? 1 : 0);
