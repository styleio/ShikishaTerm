/**
 * Does pressing a box on the settings screen hand it to the guide, and does
 * the guide's answer land in it?
 *
 *     node tools/debug/guide-fill.mjs --at <a running copy's folder>
 *
 * Needs Chrome and a copy of the app already running out of that folder with
 * the relay switched on (`tools/debug/instance.win.ps1` makes one). Nothing
 * here ships and nothing is installed.
 *
 * Why it exists. Three parts have to agree and none of them can be tested on
 * its own: the settings screen decides what a box *is* by reading what it has
 * drawn, the app holds that between the two pages, and the settings screen
 * writes the answer back with the page's own events -- a value set without
 * them leaves the page showing one thing and holding another. What is checked
 * is what a person would see: the box marked, the guide told its real name,
 * the value in the box, and the page knowing it is unsaved.
 *
 * It also checks the two refusals, because they are the whole safety of it: a
 * box holding a secret is never picked, and its value never leaves the page.
 *
 * And the way in from a phone, which is the same page in a sheet the board
 * lays over itself -- there is no window there to place anything.
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

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'guide-fill-'));
const chrome = spawn(findChrome(), [
  '--headless=new', '--remote-debugging-port=9336', '--user-data-dir=' + profile,
  '--no-first-run', 'about:blank',
], { stdio: 'ignore' });
process.on('exit', () => {
  try { execFileSync('taskkill.exe', ['/PID', String(chrome.pid), '/T', '/F'], { stdio: 'ignore' }); } catch {}
});
await sleep(1500);

const targets = await (await fetch('http://127.0.0.1:9336/json/list')).json();
const tab = targets.find((t) => t.type === 'page');
const ws = new WebSocket(tab.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
const call = (method, params) =>
  new Promise((res) => {
    const id = Math.floor(Math.random() * 1e6);
    const on = (e) => {
      const m = JSON.parse(e.data);
      if (m.id === id) { ws.removeEventListener('message', on); res(m.result); }
    };
    ws.addEventListener('message', on);
    ws.send(JSON.stringify({ id, method, params }));
  });
const js = async (expression) => {
  const r = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails));
  return r.result && r.result.value;
};
async function until(expr, what, tries = 80) {
  for (let i = 0; i < tries; i++) {
    if (await js(expr)) return;
    await sleep(250);
  }
  throw new Error('never happened: ' + what);
}

let bad = 0;
const check = (ok, what, saw) => {
  console.log((ok ? 'ok   ' : 'BAD  ') + what + (ok || saw === undefined ? '' : `  (saw ${JSON.stringify(saw)})`));
  if (!ok) bad++;
};

// The board first: opening it is what grants this browser a session
await call('Page.navigate', { url: `http://127.0.0.1:${port}/?t=${encodeURIComponent(token)}` });
await until('!!document.querySelector(".sidebtn.help")', 'the board never arrived');
await call('Page.navigate', { url: `http://127.0.0.1:${port}/cfg?t=${encodeURIComponent(token)}` });
await until('!!document.querySelector(".card input")', 'the settings never arrived');
// The page asks the app whether the ? is up; until it has an answer, pressing
// a box is meant to do nothing, and a press now would be checking the wrong
// thing rather than the thing this is for
await until('typeof guideUp !== "undefined"', 'the settings never asked about the ?');

// Nothing is picked while the ? is down, whatever is pressed
await js('fetch("/api/guide/picked", {method:"POST", headers:{"X-Token":TOKEN,"Content-Type":"application/json"}, body:"{}"})');
await sleep(300);
const upNow = await js('fetch("/api/guide/up",{headers:{"X-Token":TOKEN}}).then(r=>r.json()).then(j=>j.up)');
console.log('     the ? is ' + (upNow ? 'up' : 'down') + ' on the copy being checked');

// A box that can be picked, and one that never can
const named = await js(`(() => {
  const box = [...document.querySelectorAll(".card .row input[type=text]")].find(b => !b.disabled);
  if (!box) return null;
  box.scrollIntoView();
  box.click();
  const row = box.closest(".row");
  return {label: (row.querySelector("label")||{}).textContent, was: box.value};
})()`);
check(!!named, 'there is a box to press on the settings screen', named);

await sleep(1400);
const picked = await js('fetch("/api/guide/picked",{headers:{"X-Token":TOKEN}}).then(r=>r.json())');
if (upNow) {
  check(picked && picked.label === (named.label || '').trim(),
    'the guide is told what the box is called', picked);
  check(await js('!!document.querySelector(".card .row input.picked")'), 'the box is marked');
  check(!JSON.stringify(picked || {}).includes(named.was || '\u0000'),
    'the value of the box is not sent', picked);

  // What the guide answers lands in the box, with the page's own events
  await js('fetch("/api/guide/fill", {method:"POST", headers:{"X-Token":TOKEN,"Content-Type":"application/json"}, body: JSON.stringify({text:"written-by-the-guide"})})');
  await sleep(1600);
  check(await js('document.querySelector(".card .row input.picked").value === "written-by-the-guide"'),
    'the answer is written into the box');
  // Written with the page's own events, so the page counts it as an edit and
  // the save button says so. Set without them, the page would go on holding
  // what was there before and the button would stay quiet
  check(await js('!!document.querySelector("#savebtn.dirty")'),
    'the page counts it as an edit and the save button says so');
} else {
  console.log('     the ? is down on that copy, so picking is meant to do nothing:');
  check(picked === null, 'nothing was picked while it is down', picked);
  check(!(await js('!!document.querySelector(".card .row input.picked")')), 'no box is marked');
}

// A secret is never picked, whatever the ? is doing
const secret = await js(`(() => {
  const box = document.querySelector("input[type=password]");
  if (!box) return "none on this screen";
  box.click();
  return box.classList.contains("picked") ? "PICKED" : "left alone";
})()`);
check(secret !== 'PICKED', 'a box holding a secret is never picked', secret);

// From a phone the ? opens the same page in a sheet over the board, because
// there is no window there to place one in
await call('Emulation.setDeviceMetricsOverride', { width: 412, height: 915, deviceScaleFactor: 1, mobile: true });
await call('Page.navigate', { url: `http://127.0.0.1:${port}/?t=${encodeURIComponent(token)}` });
await until('!!document.querySelector(".sidebtn.help")', 'the board never arrived on a phone');
await js('document.querySelector(".sidebtn.help").click()');
await until('!!document.getElementById("guideframe")', 'the sheet never opened', 20).catch(() => {});
check(await js('!!document.getElementById("guideframe")'), "a phone's ? opens the sheet");
check(await js('!document.getElementById("guidewrap").hidden'), 'the sheet is shown');
check(
  await js('(document.getElementById("guideframe")||{}).src?.includes("guide?t=")'),
  'the sheet holds the same page the window places',
);
// And the frame really is served: an empty one is a sheet with nothing in it
await sleep(2500);
check(
  await js(`(() => { const f = document.getElementById("guideframe");
    try { return !!(f.contentDocument && f.contentDocument.getElementById("q")); }
    catch (e) { return "cannot see in: " + e.message; } })()`) === true,
  'the page inside the sheet is the panel',
);
await js('document.querySelector(".sidebtn.help").click()');
await sleep(600);
check(!(await js('!!document.getElementById("guideframe")')), "a phone's ? puts it away again");

ws.close();
console.log(bad ? `${bad} wrong` : 'all good');
process.exit(bad ? 1 : 0);
