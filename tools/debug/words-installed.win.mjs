/**
 * Does a page get driven from plain words by an AI installed on this PC, and
 * does the program go on moving while that AI thinks?
 *
 *     cargo build
 *     node tools/debug/words-installed.win.mjs [--ai @claude/haiku]
 *
 * Needs Windows, Node and the named AI installed and signed in. Its own copy
 * of the app (tools/debug/instance.win.ps1), its own folder and ports.
 *
 * What it does. A page with a name field and a Send button is served here, and
 * the copy is given one desk whose browser tab shows it, with the installed AI
 * as both of the desk's models and the page agreed to be sent to it. The goal
 * "type Alice in the name box and send it" is handed over the way the input
 * bar hands it over. Passing means the page's server is sent `name=Alice`.
 *
 * Meanwhile a terminal tab on the same desk prints the time five times a
 * second, and what the board says that tab shows is read every 100ms. Each
 * move waits seconds on the AI; while it did so on the program's own loop,
 * the tab's text stood still for those seconds. The longest gap between two
 * changes of it is printed and held under a limit.
 */
import http from 'node:http';
import os from 'node:os';
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const AT = path.join(os.tmpdir(), 'sk-words');
const at = process.argv.indexOf('--ai');
const AI = at > 0 ? process.argv[at + 1] : '@claude/haiku';
const TOKEN = 'words-token-0123456789abcdef';
const GOAL = 'Type Alice in the name box and send the form.';
const LIMIT_MS = 1500;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const ps = (...a) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...a], { encoding: 'utf8' });
const instance = path.join(ROOT, 'tools', 'debug', 'instance.win.ps1');
const stop = () => ps('-File', instance, '-At', AT, '-Stop');

// ── The page ──────────────────────────────────
let sent = null;
const PAGE = `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Sign-up</title></head>
<body><h1>Sign-up</h1><form action="/thanks" method="get">
<label>Name <input name="name" type="text"></label>
<button type="submit">Send</button></form></body></html>`;
const server = http.createServer((req, res) => {
  const u = new URL(req.url, 'http://x');
  if (u.pathname === '/thanks') {
    sent = u.searchParams.get('name');
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
    res.end(`<!doctype html><html><body><h1>Thanks, ${sent}. The form is sent.</h1></body></html>`);
    return;
  }
  res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
  res.end(PAGE);
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const pagePort = server.address().port;

// ── The copy ──────────────────────────────────
const scene = path.join(os.tmpdir(), 'sk-words-scene.json');
fs.writeFileSync(scene, JSON.stringify({
  remote: { sticky_token: true, fixed_token: TOKEN },
  desks: [{
    name: 'Words', id: 'words',
    browser: { choose_model: AI, words_model: AI },
    send_pages_to: AI,
    browsers: [{ id: 'form', url: `http://127.0.0.1:${pagePort}/` }],
    folders: [{ cwd: '{work}', tabs: [{ id: 'clock', name: 'clock',
      // Encoded, because a command line is split on its quotes before PowerShell
      // ever reads it
      command: 'powershell.exe -NoProfile -EncodedCommand ' + Buffer.from(
        "while ($true) { [DateTime]::Now.ToString('HH:mm:ss.fff'); Start-Sleep -Milliseconds 200 }", 'utf16le').toString('base64') }] }],
  }],
}));
const up = ps('-File', instance, '-At', AT, '-Config', scene);
const board = (up.stdout.match(/^board=(\S+)/m) || [])[1];
if (!board) { console.error(up.stdout + up.stderr); process.exit(2); }
console.log(`the copy is up at ${board}, the AI is ${AI}`);

let failed = null;
try {
  const first = await fetch(`${board}/?t=${TOKEN}`);
  const cookie = (first.headers.getSetCookie?.() || []).join('; ');
  await first.text();
  const get = async () => (await fetch(`${board}/api/state?t=${TOKEN}`, { headers: { cookie } })).text();
  const clock = (state) => ((JSON.parse(state).tabs || []).find((t) => t.name === 'clock') || {}).screen || '';
  const intent = (body) => fetch(`${board}/api/intent?t=${TOKEN}`, {
    method: 'POST', headers: { cookie, 'content-type': 'application/json' }, body: JSON.stringify(body),
  });

  // The page in front, loaded
  let tab = null;
  for (let i = 0; i < 80 && !tab; i++) {
    const s = JSON.parse(await get());
    tab = ((s.ui && s.ui.tabs) || s.tabs || []).find((t) => t.id === 'form' || t.name === 'form');
    if (!tab) await sleep(250);
  }
  if (!tab) throw new Error('the copy has no tab called form');
  await intent({ kind: 'select', tab: tab.index });
  await sleep(3000);

  // How still the clock stands with nothing running, for the line below
  // to be read against
  const watch = async (ms, until = () => false) => {
    const from = Date.now();
    let last = clock(await get()), lastChange = Date.now(), longest = 0;
    if (!last) throw new Error('the clock tab shows nothing');
    while (!until() && Date.now() - from < ms) {
      await sleep(100);
      const now = clock(await get());
      if (now !== last) {
        longest = Math.max(longest, Date.now() - lastChange);
        lastChange = Date.now();
        last = now;
      }
    }
    return longest;
  };
  const idle = await watch(8000);
  console.log(`with nothing running, the clock tab stood still for up to ${idle}ms`);

  // The goal, and the watch on the clock while it is carried out
  await intent({ kind: 'words', on: true, goal: GOAL });
  const started = Date.now();
  const longest = await watch(180000, () => sent !== null);
  console.log(`sent: ${JSON.stringify(sent)} after ${((Date.now() - started) / 1000).toFixed(1)}s`);
  console.log(`the longest the clock tab stood still: ${longest}ms`);
  if (sent !== 'Alice') failed = `the form was not sent with Alice (${JSON.stringify(sent)})`;
  else if (longest > LIMIT_MS) failed = `the clock tab stood still for ${longest}ms`;
} catch (e) {
  failed = String(e && e.stack || e);
} finally {
  stop();
  server.close();
}
if (failed) {
  console.error('FAIL: ' + failed + `\n  (the copy's log is under ${path.join(AT, 'app', 'logs')})`);
  process.exit(1);
}
console.log('PASS');
