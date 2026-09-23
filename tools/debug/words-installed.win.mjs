/**
 * Does a page get driven from plain words by an AI installed on this PC, and
 * does the program go on moving while that AI thinks?
 *
 *     cargo build
 *     node tools/debug/words-installed.win.mjs [--ai @claude/haiku]
 *     JEV_API_KEY=... node tools/debug/words-installed.win.mjs --choose jev
 *     node tools/debug/words-installed.win.mjs --ai assistant
 *     node tools/debug/words-installed.win.mjs --ai assistant --unagreed
 *
 * `--url <address> --goal <words>` drives a real page instead of the form,
 * for as long as `--seconds` (default 90), and prints what the run said on
 * its way. Nothing is checked but that the program kept moving.
 *
 * `--unagreed` writes no agreement to send pages. The goal is refused, and
 * then sent again the way "Agree and run" beside that refusal sends it: the
 * run has to start, and the agreement has to be in the settings file.
 *
 * `--ai assistant` chooses no model at all: the desk drives its pages with
 * the assistant AI (the first installed, Claude Code here), and only the
 * agreement to send pages to it is written.
 *
 * `--choose jev` makes the decision model Jev (TypeSafe) and leaves only the
 * writing to the installed AI. The key is read from the environment, never
 * from the command line (every process on the machine can read that), and
 * the copy's settings and data are deleted when the run ends.
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
const UNSET = AI === 'assistant';
const UNAGREED = process.argv.includes('--unagreed');
const TOKEN = 'words-token-0123456789abcdef';
const JEV = process.argv.includes('--choose') && process.argv[process.argv.indexOf('--choose') + 1] === 'jev';
if (JEV && !process.env.JEV_API_KEY) { console.error('--choose jev needs JEV_API_KEY'); process.exit(2); }
const CHOOSE = JEV ? 'jev/jev-latest' : AI;
const arg = (name) => { const i = process.argv.indexOf(name); return i > 0 ? process.argv[i + 1] : undefined; };
const URL_ = arg('--url');
const GOAL = arg('--goal') || 'Type Alice in the name box and send the form.';
const SECONDS = Number(arg('--seconds') || 90);
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
    browser: UNSET ? {} : { choose_model: CHOOSE, words_model: AI },
    // Agreed to, the way the settings write it: each name once, in order
    send_pages_to: UNAGREED ? undefined : UNSET ? '@claude' : [...new Set([CHOOSE, AI])].sort().join(' + '),
    providers: JEV ? { jev: { base_url: 'https://api.typesafe.ai/v1/systemone', speaks: 'choice',
      models: ['jev-latest'], api_key: process.env.JEV_API_KEY } } : {},
    browsers: [{ id: 'form', url: URL_ || `http://127.0.0.1:${pagePort}/` }],
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
console.log(`the copy is up at ${board}, deciding: ${CHOOSE}, writing: ${AI}`);

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
  if (UNAGREED) {
    await sleep(1500);
    const log = fs.readFileSync(path.join(AT, 'app', 'logs', 'hooks.log'), 'utf8');
    if (!/words: not started/.test(log)) throw new Error('an unagreed page was not refused');
    if (sent !== null) throw new Error('an unagreed page was driven');
    console.log('refused until agreed; agreeing');
    await intent({ kind: 'words', on: true, goal: GOAL, agree: true });
  }
  const started = Date.now();
  const longest = await watch(URL_ ? SECONDS * 1000 : 180000, () => sent !== null);
  if (URL_) {
    const said = fs.readFileSync(path.join(AT, 'app', 'logs', 'hooks.log'), 'utf8')
      .split(/\r?\n/).filter((l) => /words|Result code/.test(l));
    console.log(said.join('\n'));
  }
  console.log(`sent: ${JSON.stringify(sent)} after ${((Date.now() - started) / 1000).toFixed(1)}s`);
  console.log(`the longest the clock tab stood still: ${longest}ms`);
  if (UNAGREED) {
    const cfg = JSON.parse(fs.readFileSync(path.join(AT, 'app', 'config', 'config.json'), 'utf8').replace(/^﻿/, ''));
    const agreed = cfg.desks[0].send_pages_to;
    console.log(`the desk now agrees to: ${agreed}`);
    if (agreed !== '@claude') failed = `the agreement was written as ${JSON.stringify(agreed)}`;
  }
  if (URL_) { /* a real page: what it said is the result */ }
  else if (sent !== 'Alice') failed = `the form was not sent with Alice (${JSON.stringify(sent)})`;
  else if (longest > LIMIT_MS) failed = `the clock tab stood still for ${longest}ms`;
} catch (e) {
  failed = String(e && e.stack || e);
} finally {
  stop();
  server.close();
  // The scene and the copy's settings may hold a key -- and so may the copy
  // of those settings the app backs up when it starts on an older file
  fs.rmSync(scene, { force: true });
  for (const d of ['config', 'data']) fs.rmSync(path.join(AT, 'app', d), { recursive: true, force: true });
}
if (failed) {
  console.error('FAIL: ' + failed + `\n  (the copy's log is under ${path.join(AT, 'app', 'logs')})`);
  process.exit(1);
}
console.log('PASS');
