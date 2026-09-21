/**
 * Does the board come up at all? Asked of the page itself, outside the app.
 *
 *     node tools/check-board.mjs            the window's page and a phone's
 *     node tools/check-board.mjs --keep     leave the pages behind to look at
 *
 * Why it exists. The window shows a splash until the app hands the page its
 * first board state, and the page takes that state in one function
 * (`window.__state`). A throw anywhere in that function is invisible: the app
 * runs it through a call whose failure is reported back to the app and nobody
 * else, so the screen just keeps spinning and the log says nothing. A page that
 * cannot take its first state is an app that never starts -- and the way that
 * has happened is ordinary editing: a name declared inside that function which
 * the top of the same function already reads (`const holding` under a `holding`
 * from the page's own scope), a typo in a branch that only a certain state
 * reaches, a call to something that was renamed.
 *
 * So the page is written out as it is served (src/bin/page_dump.rs), opened in
 * a headless Chrome, handed the state the app hands it (the same `UiState` the
 * app sends, from the same binary), and asked two things: did anything throw,
 * and did the splash come down. Nothing here is a fixture: the state comes from
 * the app's own type, so a field added to it is in this check the day it lands.
 *
 * It needs Chrome (CHROME says where, if it is somewhere unusual) and cargo.
 * Nothing here ships; it runs in CI and on the machine of whoever is editing
 * the page.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

const ROOT = path.resolve(import.meta.dirname, '..');
const OUT = path.join(ROOT, 'target', 'board-check');
const PORT = 9336;
const KEEP = process.argv.includes('--keep');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(1); };

/** Chrome, wherever this machine keeps it. CHROME says so outright. */
function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  const guesses = process.platform === 'win32'
    ? [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA]
      .filter(Boolean)
      .map((base) => path.join(base, 'Google/Chrome/Application/chrome.exe'))
    : process.platform === 'darwin'
      ? ['/Applications/Google Chrome.app/Contents/MacOS/Google Chrome']
      : ['/usr/bin/google-chrome', '/usr/bin/chromium', '/usr/bin/chromium-browser'];
  const found = guesses.find((p) => fs.existsSync(p));
  if (!found) die('Chrome was not found. Say where it is with CHROME=<path>.');
  return found;
}

/** cargo is installed per-user, and a shell started without it stays without it. */
function findCargo() {
  const named = process.platform === 'win32' ? 'cargo.exe' : 'cargo';
  const beside = path.join(os.homedir(), '.cargo', 'bin', named);
  if (fs.existsSync(beside)) return beside;
  const where = spawnSync(process.platform === 'win32' ? 'where' : 'which', ['cargo']);
  if (where.status === 0) return 'cargo';
  return die('cargo was not found; install rustup first');
}

const cargo = findCargo();
/** Ask the app itself for something: the page as it serves it, or a state. */
function fromApp(args) {
  const made = spawnSync(cargo, ['run', '--quiet', '--bin', 'page_dump', '--', ...args],
    { cwd: ROOT, maxBuffer: 1 << 28 });
  if (made.status !== 0) die('page_dump ' + args.join(' ') + ' failed: ' + made.stderr);
  return made.stdout.toString('utf8');
}

/** A talking connection to one tab of a headless Chrome. */
async function connect(chrome) {
  const proc = spawn(chrome, [
    '--headless=new', '--remote-debugging-port=' + PORT,
    '--user-data-dir=' + path.join(OUT, 'chrome-profile'),
    '--no-first-run', '--no-default-browser-check', '--hide-scrollbars',
    'about:blank',
  ], { stdio: 'ignore' });
  let list;
  for (let i = 0; i < 80 && !list; i++) {
    try {
      list = await (await fetch('http://127.0.0.1:' + PORT + '/json/list')).json();
    } catch {
      await sleep(250);
    }
  }
  if (!list) die('Chrome never answered on its debugging port');
  const ws = new WebSocket(list.find((t) => t.type === 'page').webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r, { once: true }));
  let id = 0;
  const waiting = new Map();
  // Everything the page throws where nobody catches it. The page reports its
  // own failures to the app, which cannot be listened to from here, so they are
  // taken from the browser instead
  const thrown = [];
  ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data);
    if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); return; }
    if (m.method === 'Runtime.exceptionThrown') {
      const d = m.params.exceptionDetails;
      thrown.push((d.exception && (d.exception.description || d.exception.value)) || d.text);
    }
  });
  const send = (method, params = {}) => new Promise((res, rej) => {
    const n = ++id;
    waiting.set(n, (m) => (m.error
      ? rej(new Error(method + ': ' + JSON.stringify(m.error)))
      : res(m.result)));
    ws.send(JSON.stringify({ id: n, method, params }));
  });
  const run = async (expression) => {
    const r = await send('Runtime.evaluate',
      { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) {
      throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    }
    return r.result.value;
  };
  return { send, run, thrown, stop: () => { ws.close(); proc.kill(); } };
}

fs.mkdirSync(OUT, { recursive: true });
// The state first: it is the same for every page, and asking the app for it
// once keeps a build out of the middle of the checks
const STATE = fromApp(['state']);
const chrome = await connect(findChrome());
await chrome.send('Page.enable');
await chrome.send('Runtime.enable');
// The page talks to the app through a bridge that is not here. A place that
// swallows what it is told, installed before the page's own script runs --
// that script gives up on its first throw, and everything below it with it
await chrome.send('Page.addScriptToEvaluateOnNewDocument',
  { source: 'window.ipc = { postMessage(){} };' });

let bad = 0;
const fail = (what, why) => { console.error('  FAILED ' + what + ': ' + why); bad += 1; };

// Both pages, in both languages: the window's, and the one a phone is served.
// The languages because the wording is baked into the page, and a page that
// only comes up in English is a page that does not come up here
for (const side of ['window', 'remote']) {
  for (const lang of ['en', 'ja']) {
    const what = side + '/' + lang;
    const file = path.join(OUT, 'page.' + lang + '.' + side + '.html');
    fs.writeFileSync(file, fromApp(side === 'remote' ? [lang, 'remote'] : [lang]));
    chrome.thrown.length = 0;
    await chrome.send('Emulation.setDeviceMetricsOverride',
      { width: side === 'remote' ? 390 : 1280, height: side === 'remote' ? 820 : 860,
        deviceScaleFactor: 1, mobile: side === 'remote' });
    await chrome.send('Page.navigate', { url: pathToFileURL(file).href });
    await sleep(900);
    // The page's own script has to have run to its end: it says it is up on
    // its last line, and everything the app then sends goes to what that
    // script defined. A page that stopped halfway still looks like a page
    const up = await chrome.run('typeof window.__state === "function" && typeof window.__screen === "function"');
    if (!up) fail(what, 'the page never finished starting (no __state)');
    if (chrome.thrown.length) fail(what, 'it threw while starting: ' + chrome.thrown.join(' | '));

    chrome.thrown.length = 0;
    // The first state, exactly as the app hands it over
    try {
      await chrome.run('window.__state(' + JSON.stringify(STATE) + ')');
    } catch (e) {
      fail(what, 'the first state threw: ' + String(e.message).split('\n')[0]);
    }
    await sleep(250);
    if (chrome.thrown.length) fail(what, 'the first state threw: ' + chrome.thrown.join(' | '));
    // What a person sees: the splash is the app saying "not yet", and it comes
    // down when, and only when, the board has been drawn
    const splash = await chrome.run('(document.getElementById("splash")||{}).hidden === true');
    if (!splash) fail(what, 'the splash stayed up after the board arrived');
    // And the terminal's own contents, which arrive by their own call
    chrome.thrown.length = 0;
    try {
      await chrome.run('window.__screen("hello")');
    } catch (e) {
      fail(what, 'the screen threw: ' + String(e.message).split('\n')[0]);
    }
    if (chrome.thrown.length) fail(what, 'the screen threw: ' + chrome.thrown.join(' | '));
    if (!bad) console.log('  ok ' + what);
  }
}

chrome.stop();
// The pages, not the browser's own folder: Chrome is still letting go of its
// files as this runs, and a check that failed on tidying up would be a check
// that fails for no reason at all
if (!KEEP) {
  for (const f of fs.readdirSync(OUT)) {
    if (f.endsWith('.html')) { try { fs.rmSync(path.join(OUT, f)); } catch {} }
  }
}
if (bad) {
  console.error(bad + ' check(s) failed: the board does not come up');
  process.exit(1);
}
console.log('the board comes up');
