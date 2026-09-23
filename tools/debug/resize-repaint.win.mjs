/**
 * What is on screen straight after the size changes -- and whether it is still
 * wrong a moment later.
 *
 *     node tools/debug/resize-repaint.win.mjs
 *
 * Needs Windows, Chrome and a build (`cargo build --bin SHIKISHA-TERM`). It
 * lays out a copy of the app of its own under D:\ShikishaTerm-resize (or
 * %TEMP% where there is no D:), runs `ruler-tui.mjs` in its tabs, opens the
 * board in a headless Chrome, and changes the size the board reports -- which
 * is the same road a window being dragged takes. Every process it stops is
 * matched by FULL PATH.
 *
 * Why it exists. A screen that is drawn wrong and stays wrong until somebody
 * presses a key is the worst thing a terminal can do, and it is invisible to
 * every test that reads state rather than pixels: the app's own idea of the
 * size is right, the program's is right, and the picture between them is
 * torn. `ruler-tui.mjs` says on every row how wide and how tall it thinks it
 * is, so a reading of the board answers whose idea is stale.
 *
 * It measures two things, twice each -- a moment after the change and again
 * once everything has settled:
 *   - a program that redraws when it is told (the ordinary case)
 *   - a program that never redraws on its own (--lazy), where the picture can
 *     only come from what we already hold
 *
 * Exit code 0 when every check passed.
 */
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SRC = path.resolve(fileURLToPath(new URL('../..', import.meta.url)));
const TUI = path.join(SRC, 'tools', 'debug', 'ruler-tui.mjs').replace(/\\/g, '/');
const LAB = process.env.SHIKISHA_RESIZE_LAB
  || (fs.existsSync('D:\\') ? 'D:\\ShikishaTerm-resize' : path.join(os.tmpdir(), 'ShikishaTerm-resize'));
const PORT = 8793;
const TOK = 'labtoken0123456789abcdef';
const BASE = `http://127.0.0.1:${PORT}`;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let fails = 0, checks = 0, seen = null;
function ok(cond, what) {
  checks++;
  if (cond) { console.log('    ok    ' + what); return; }
  fails++;
  console.log('    FAIL  ' + what);
  if (seen) console.log('          ' + JSON.stringify(seen));
}

const ps = (cmd) => execFileSync('powershell', ['-NoProfile', '-Command', cmd], { encoding: 'utf8' }).trim();
function stopLab() {
  ps(`Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue |` +
     ` Where-Object { $_.Path -like '${LAB}\\*' } | Stop-Process -Force -ErrorAction SilentlyContinue`);
}

function lay() {
  fs.rmSync(LAB, { recursive: true, force: true });
  for (const d of ['', 'config', 'logs', 'work\\a']) fs.mkdirSync(path.join(LAB, d), { recursive: true });
  for (const n of ['SHIKISHA-TERM.exe', 'conpty.dll', 'OpenConsole.exe']) {
    const from = path.join(SRC, 'target', 'debug', n);
    if (fs.existsSync(from)) fs.copyFileSync(from, path.join(LAB, n));
  }
  for (const d of ['lang', 'profiles']) {
    if (fs.existsSync(path.join(SRC, d))) fs.cpSync(path.join(SRC, d), path.join(LAB, d), { recursive: true });
  }
  const cwd = (LAB + '\\work\\a').replace(/\\/g, '/');
  fs.writeFileSync(path.join(LAB, 'config', 'config.json'), JSON.stringify({
    language: 'en', resident: false,
    remote: { enabled: true, bind: '127.0.0.1', port: PORT, sticky_token: true, fixed_token: TOK },
    desks: [{ id: 'default', name: 'DESK', folders: [{ name: 'a', cwd, tabs: [
      { name: 'live', id: 'a-live', command: `node ${TUI}` },
      { name: 'lazy', id: 'a-lazy', command: `node ${TUI} --lazy` },
      { name: 'beside', id: 'a-beside', command: `node ${TUI}` }] }] }],
  }, null, 2), 'utf8');
}

let cookie = '';
async function start() {
  spawn(path.join(LAB, 'SHIKISHA-TERM.exe'), [], { cwd: LAB, detached: true, stdio: 'ignore' }).unref();
  for (let i = 0; i < 30; i++) {
    await sleep(1000);
    try {
      const r = await fetch(`${BASE}/?t=${TOK}`);
      if (!r.ok) continue;
      cookie = (r.headers.getSetCookie ? r.headers.getSetCookie() : []).map((c) => c.split(';')[0]).join('; ');
      await sleep(4000);
      return true;
    } catch (e) { /* not up yet */ }
  }
  return false;
}
const state = async () => {
  const st = await (await fetch(`${BASE}/api/state?t=${TOK}`, { headers: { cookie } })).json();
  return st.ui || st;
};
const say = async (body) => {
  await fetch(`${BASE}/api/intent?t=${TOK}`, {
    method: 'POST', headers: { 'content-type': 'application/json', cookie }, body: JSON.stringify(body) });
  await sleep(2500);
};

function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  for (const base of [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA]) {
    if (!base) continue;
    const p = path.join(base, 'Google', 'Chrome', 'Application', 'chrome.exe');
    if (fs.existsSync(p)) return p;
  }
  throw new Error('no Chrome found; set CHROME');
}

// What the board is showing, pane by pane: the rows the program drew, how
// long each is, and what size the program itself says it has. The pane in
// front is drawn by the full renderer (#screen); the others hold a picture
const LOOK = `(() => {
  const read = (text) => {
    text = (text || '').replace(/\\u00a0/g, ' ');
    const lines = text.split('\\n').filter(l => /^R\\d\\d/.test(l));
    const said = /C=(\\d+) R=(\\d+) D=(\\d+) ([VZ])/.exec(text);
    return {
      // Every row the program drew ends in a bar at its right edge. Two
      // widths at once means two drawings are showing through each other
      widths: [...new Set(lines.map(l => l.replace(/\\s+$/, '').length))].sort((a, b) => a - b),
      ends: lines.length ? lines.every(l => l.replace(/\\s+$/, '').endsWith('|')) : false,
      rowsDrawn: lines.length,
      saidCols: said ? Number(said[1]) : 0,
      saidRows: said ? Number(said[2]) : 0,
      drawn: said ? Number(said[3]) : 0,
      who: said ? (said[4] === 'Z' ? 'LAZY' : 'LIVE') : '?',
    };
  };
  const front = document.getElementById('screen');
  return {
    front: read(front ? front.innerText : ''),
    panes: [...document.querySelectorAll('#panes .pane')].map(el => {
      const focused = el.classList.contains('focused');
      const src = focused ? front : el.querySelector('.pscreen');
      return Object.assign(read(src ? src.innerText : ''), {
        id: el.dataset.pid, focused,
        w: Math.round(el.getBoundingClientRect().width),
      });
    }),
  };
})()`;

async function main() {
  console.log('lab: ' + LAB);
  stopLab();
  await sleep(1000);
  lay();
  if (!await start()) { console.log('the lab never came up'); return 2; }

  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'resize-'));
  const chrome = spawn(findChrome(), [
    '--headless=new', '--remote-debugging-port=9335', '--user-data-dir=' + profile,
    '--window-size=1200,860', '--no-first-run', 'about:blank'], { stdio: 'ignore' });
  await sleep(2500);
  const { attach } = await import(new URL('../../.private/doc/proto/webrtc-http/cdp.mjs', import.meta.url).href);
  const cdp = await attach(9335);
  const look = async () => {
    seen = (await cdp.call('Runtime.evaluate', { returnByValue: true, expression: LOOK })).result.result.value;
    return seen;
  };
  const size = async (w, h) => {
    await cdp.call('Emulation.setDeviceMetricsOverride',
      { width: w, height: h, deviceScaleFactor: 1, mobile: false });
  };

  try {
    await cdp.call('Page.enable');
    await cdp.call('Runtime.enable');
    await size(1200, 860);
    await cdp.call('Page.navigate', { url: `${BASE}/?t=${TOK}` });
    await sleep(10000);
    await say({ kind: 'select', tab: 1 });
    await sleep(3000);

    for (const [name, tab, live] of [
      ['a program that redraws when told', 1, true],
      ['a program that never redraws', 2, false],
    ]) {
      await say({ kind: 'select', tab });
      await size(1200, 860);
      await sleep(4000);
      console.log('  ' + name);
      let v = (await look()).front;
      ok(v.rowsDrawn > 5, 'it is on screen to begin with (' + v.rowsDrawn + ' rows, ' + v.who + ')');
      let was = v.saidCols;

      for (const [to, w, h] of [['narrower', 760, 600], ['wider again', 1200, 860]]) {
        // The size changes under it, the way a window being dragged does
        await size(w, h);
        await sleep(900);
        console.log('      a moment after getting ' + to + ': ' + JSON.stringify((await look()).front));
        // ...and then it is left alone. Nothing is pressed: the whole question
        // is what somebody sees once they have stopped touching it
        await sleep(6000);
        v = (await look()).front;
        ok(v.rowsDrawn > 5, to + ': there is still a screen');
        ok(v.widths.length === 1,
          to + ': one drawing on screen, not two through each other: ' + JSON.stringify(v.widths));
        if (!live) continue;
        ok(v.ends, to + ': every row ends where the screen ends');
        ok(v.saidCols !== was, to + ': the program was told the width changed (' + was + ' -> ' + v.saidCols + ')');
        ok(v.saidRows === v.rowsDrawn,
          to + ': it drew as many rows as it says it has (' + v.saidRows + ' vs ' + v.rowsDrawn + ')');
        was = v.saidCols;
      }
    }

    // ...and the same again with the area divided. A pane is where a size
    // changes most violently -- half the width in one press -- and a picture
    // of a pane nobody is typing into is built by a different road (PaneRelay)
    // from the one in front, so it can go stale on its own
    console.log('  a division, and then the screen changing size under it');
    // This round is about what a resize does to a division, so the division
    // is drawn whatever the width: the narrow-screen question has its own
    // tool (split-narrow.win.mjs) and answering it here would hide the panes
    await cdp.call('Runtime.evaluate', {expression: "localStorage.setItem('splitNarrow','both')"});
    await say({ kind: 'select', tab: 1 });
    await size(1200, 860);
    await sleep(4000);
    const whole = (await look()).front.saidCols;
    await say({ kind: 'splitpane', id: (await state()).panes.panes[0].id, down: false });
    const empty = (await state()).panes.panes.find((p) => p.surface === 0);
    if (empty) { await say({ kind: 'focuspane', id: empty.id }); await say({ kind: 'select', tab: 3 }); }
    await sleep(6000);
    let v = await look();
    ok(v.panes.length === 2, 'the area is divided in two');
    for (const p of v.panes) {
      ok(p.rowsDrawn > 5, (p.focused ? 'the pane in front' : 'the pane beside it') + ' has a screen');
      ok(p.widths.length === 1,
        (p.focused ? 'the pane in front' : 'the pane beside it') + ': one drawing, not two: ' + JSON.stringify(p.widths));
      ok(p.saidCols > 0 && p.saidCols < whole,
        (p.focused ? 'the pane in front' : 'the pane beside it') + ' was told its half of the width (' + p.saidCols + ' of ' + whole + ')');
      ok(p.saidCols === p.widths[0],
        (p.focused ? 'the pane in front' : 'the pane beside it') + ' drew to the width it was told (' + p.saidCols + ' vs ' + JSON.stringify(p.widths) + ')');
    }

    // The screen changes size with the division standing, and nothing is
    // pressed afterwards
    await size(760, 700);
    await sleep(7000);
    v = await look();
    for (const p of v.panes) {
      const who = p.focused ? 'the pane in front' : 'the pane beside it';
      ok(p.widths.length === 1, who + ', narrower: one drawing, not two: ' + JSON.stringify(p.widths));
      ok(p.ends, who + ', narrower: every row ends where the pane ends');
      ok(p.saidCols === p.widths[0], who + ', narrower: drawn to the width it was told (' + p.saidCols + ' vs ' + JSON.stringify(p.widths) + ')');
    }

    // ...and the division goes away again
    await say({ kind: 'closepane', id: Number(v.panes.find((p) => !p.focused).id) });
    await size(1200, 860);
    await sleep(7000);
    v = await look();
    ok(v.panes.length === 1, 'one pane again');
    ok(v.front.saidCols === whole, 'it has the whole width back (' + v.front.saidCols + ' vs ' + whole + ')');
    ok(v.front.ends && v.front.widths.length === 1, 'and one drawing across it: ' + JSON.stringify(v.front.widths));
  } finally {
    cdp.close();
    chrome.kill();
    await sleep(500);
    fs.rmSync(profile, { recursive: true, force: true });
    stopLab();
  }
  console.log(`\n${checks - fails}/${checks} checks passed`);
  return fails ? 1 : 0;
}

process.exit(await main());
