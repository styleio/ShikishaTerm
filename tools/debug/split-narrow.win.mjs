/**
 * A division on a screen with no room for it: what a phone is actually shown.
 *
 *     node tools/debug/split-narrow.win.mjs
 *
 * Needs Windows, Chrome and a build (`cargo build --bin SHIKISHA-TERM`). It
 * lays out a copy of the app of its own under D:\ShikishaTerm-narrow (or
 * %TEMP% where there is no D:), starts it, divides a folder through the
 * board's HTTP door, and then looks at the board from a headless Chrome the
 * size of a phone. Every process it stops is matched by FULL PATH.
 *
 * Why it exists. The rule for a small screen is not "is this a phone" -- a
 * tablet held upright and the same tablet turned are two different answers,
 * and a division downwards never runs out of room at all. That arithmetic
 * lives on the page, in pixels, so nothing the app can be asked over HTTP
 * shows whether it is right. This stands in for the hand holding the phone:
 * it reads the bar, presses its buttons, and turns the screen.
 *
 * What it checks, at 390x844 (a phone upright), then at 900x600 (turned):
 *   - a split row is listed at every size -- it is not hidden from a phone
 *   - a division across the middle, with no room, asks once before drawing it
 *   - "show both" draws both, "one at a time" draws the one in front, full
 *     width, with the way to the others beside it
 *   - the answer is remembered, and is reversible from the same bar
 *   - turning the screen wide enough draws the division whatever was answered
 *   - a division downwards is drawn as it is, and is never asked about
 *
 * Exit code 0 when every check passed.
 */
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SRC = path.resolve(fileURLToPath(new URL('../..', import.meta.url)));
const LAB = process.env.SHIKISHA_NARROW_LAB
  || (fs.existsSync('D:\\') ? 'D:\\ShikishaTerm-narrow' : path.join(os.tmpdir(), 'ShikishaTerm-narrow'));
const PORT = 8792;
const TOK = 'labtoken0123456789abcdef';
const BASE = `http://127.0.0.1:${PORT}`;
const EXE = path.join(LAB, 'SHIKISHA-TERM.exe');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let fails = 0, checks = 0;
// The last thing the page was seen to be, printed under any check that fails:
// a check that only says "no" costs another run of this to find out why
let seen = null;
function ok(cond, what) {
  checks++;
  if (cond) { console.log('    ok    ' + what); return; }
  fails++;
  console.log('    FAIL  ' + what);
  if (seen) console.log('          ' + JSON.stringify(seen));
}

// ---- the copy of the app this drives ---------------------------------------

const ps = (cmd) => execFileSync('powershell', ['-NoProfile', '-Command', cmd], { encoding: 'utf8' }).trim();

// Only ever this lab's own, by full path: by name it would catch a real one
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
      { name: 'a1', id: 'a-1', command: 'cmd' }, { name: 'a2', id: 'a-2', command: 'cmd' }] }] }],
  }, null, 2), 'utf8');
}

let cookie = '';
async function start() {
  spawn(EXE, [], { cwd: LAB, detached: true, stdio: 'ignore' }).unref();
  for (let i = 0; i < 30; i++) {
    await sleep(1000);
    try {
      const r = await fetch(`${BASE}/?t=${TOK}`);
      if (!r.ok) continue;
      cookie = (r.headers.getSetCookie ? r.headers.getSetCookie() : [])
        .map((c) => c.split(';')[0]).join('; ');
      await sleep(3000);
      return true;
    } catch (e) { /* not up yet */ }
  }
  return false;
}

const say = async (body) => {
  await fetch(`${BASE}/api/intent?t=${TOK}`, {
    method: 'POST', headers: { 'content-type': 'application/json', cookie },
    body: JSON.stringify(body),
  });
  await sleep(2500);
};
const state = async () => {
  const st = await (await fetch(`${BASE}/api/state?t=${TOK}`, { headers: { cookie } })).json();
  return st.ui || st;
};

// ---- the phone ------------------------------------------------------------

function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  for (const base of [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA]) {
    if (!base) continue;
    const p = path.join(base, 'Google', 'Chrome', 'Application', 'chrome.exe');
    if (fs.existsSync(p)) return p;
  }
  throw new Error('no Chrome found; set CHROME');
}

const LOOK = `(() => {
  const bar = document.getElementById('narrowsplit');
  const on = !!(bar && !bar.hidden);
  const panes = [...document.querySelectorAll('#panes .pane')];
  return {
    bar: on ? bar.textContent.trim() : null,
    buttons: on ? [...bar.querySelectorAll('.nbtn')].map(b => b.textContent.trim()) : [],
    count: on ? (bar.querySelector('.ncount') || {}).textContent || '' : '',
    panes: panes.map(el => ({
      id: el.dataset.pid,
      w: Math.round(el.getBoundingClientRect().width),
      h: Math.round(el.getBoundingClientRect().height),
      focused: el.classList.contains('focused'),
    })),
    rows: [...document.querySelectorAll('#tabs .tab.intab')].map(el => el.textContent.trim()),
    width: Math.round((document.getElementById('panes') || {getBoundingClientRect:()=>({width:0})})
      .getBoundingClientRect().width),
  };
})()`;

async function main() {
  console.log('lab: ' + LAB);
  stopLab();
  await sleep(1000);
  lay();
  if (!await start()) { console.log('the lab never came up'); return 2; }

  // A division across the middle, both halves filled: the shape a narrow
  // screen has no room for
  await say({ kind: 'select', tab: 1 });
  await say({ kind: 'splitpane', id: 1, down: false });
  let ui = await state();
  const empty = (ui.panes.panes || []).find((p) => p.surface === 0);
  if (empty) { await say({ kind: 'focuspane', id: empty.id }); await say({ kind: 'select', tab: 2 }); }
  ui = await state();
  const split = (ui.tabs || []).find((t) => t.kind === 'split');
  ok(!!split, 'the app made a split row to look at');
  ok((ui.panes.panes || []).length === 2, 'it holds two panes');
  if (!split) { stopLab(); return 1; }

  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'narrow-phone-'));
  const chrome = spawn(findChrome(), [
    '--headless=new', '--remote-debugging-port=9334', '--user-data-dir=' + profile,
    '--window-size=390,844', '--no-first-run', 'about:blank',
  ], { stdio: 'ignore' });
  await sleep(2500);
  const { attach } = await import(new URL('../../.private/doc/proto/webrtc-http/cdp.mjs', import.meta.url).href);
  const cdp = await attach(9334);
  const look = async () => {
    seen = (await cdp.call('Runtime.evaluate', { returnByValue: true, expression: LOOK }))
      .result.result.value;
    return seen;
  };
  const press = async (js) => {
    await cdp.call('Runtime.evaluate', { returnByValue: true, expression: js });
    await sleep(1200);
  };
  const size = async (w, h) => {
    await cdp.call('Emulation.setDeviceMetricsOverride',
      { width: w, height: h, deviceScaleFactor: 1, mobile: w < h });
    await sleep(1200);
  };

  try {
    await cdp.call('Page.enable');
    await cdp.call('Runtime.enable');
    await size(390, 844);
    await cdp.call('Page.navigate', { url: `${BASE}/?t=${TOK}` });
    await sleep(9000);

    console.log('  a phone held upright (390x844), never asked before');
    // A folder's tabs start folded into their box, on every screen. Opening it
    // is how they are ever seen, and the question here is what is in it
    await press(`(document.querySelector('#tabs .bundle.away') || {click(){}}).click()`);
    let v = await look();
    ok(v.rows.some((r) => r.includes(split.name)), 'the split row is listed on a phone');
    ok(v.bar !== null, 'it says the screen is narrow for this division');
    ok(v.buttons.length === 2, 'and offers both ways of reading it, not one');
    ok(v.panes.length === 1, 'until it is answered, the pane in front has the screen');
    ok(v.panes.length === 1 && v.panes[0].w >= v.width - 2, '...the whole of it');

    console.log('  "show both"');
    await press(`[...document.querySelectorAll('#narrowsplit .nbtn')][0].click()`);
    v = await look();
    ok(v.panes.length === 2, 'both panes are drawn on the phone');
    ok(v.panes.every((p) => p.w < 260), 'each of them narrow, which is what was asked for');
    ok(v.bar !== null && v.buttons.length === 1, 'the bar stays, offering the way back');

    console.log('  "one at a time"');
    await press(`[...document.querySelectorAll('#narrowsplit .nbtn')].pop().click()`);
    v = await look();
    ok(v.panes.length === 1, 'one pane again');
    ok(v.count.replace(/\s/g, '') === '1/2' || v.count.replace(/\s/g, '') === '2/2',
      'the bar says which of them: ' + JSON.stringify(v.count));
    const before = v.panes[0].id;
    await press(`[...document.querySelectorAll('#narrowsplit .nstep')].pop().click()`);
    v = await look();
    ok(v.panes.length === 1 && v.panes[0].id !== before, 'the arrow walks to the pane beside it');

    console.log('  the same phone, turned (900x600)');
    await size(900, 600);
    v = await look();
    ok(v.bar === null, 'nothing is said: there is room now');
    ok(v.panes.length === 2, 'and the division is drawn, whatever was answered before');

    console.log('  turned back (390x844)');
    await size(390, 844);
    v = await look();
    ok(v.panes.length === 1, 'one at a time again -- the answer was remembered');
    ok(v.bar !== null, 'and the bar came back with it');

    console.log('  a division downwards, on the same phone');
    await press(`localStorage.removeItem('splitNarrow')`);
    await say({ kind: 'closepane', id: (await state()).panes.panes[1].id });
    await say({ kind: 'splitpane', id: (await state()).panes.panes[0].id, down: true });
    await sleep(3000);
    v = await look();
    ok(v.panes.length === 2, 'both halves are drawn');
    ok(v.panes.every((p) => p.w >= v.width - 2), 'one above the other');
    ok(v.bar === null, 'and nothing was asked: a height cannot run out this way');
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
