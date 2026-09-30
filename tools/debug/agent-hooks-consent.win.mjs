/**
 * The question asked as the program starts about the AI CLIs' hooks, answered
 * the three ways it can be, each followed by a second start.
 *
 *   on     the hooks go into each CLI's settings file, Codex is told they are
 *          approved (asked of Codex itself), and the next start does not ask
 *   off    nothing is written, and the next start does not ask
 *   later  nothing is written, and the next start asks again
 *
 *     cargo build
 *     node tools/debug/agent-hooks-consent.win.mjs [--only=on|off|later] [--keep]
 *
 * Needs Windows, Node and `codex` on PATH (it is started only to say which
 * hooks it runs; nothing is sent to any AI). Isolated all the way down: a
 * home folder of its own (USERPROFILE), so the .codex and .claude written into
 * are this run's and never the person's, and its own LOCALAPPDATA.
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const ONLY = (process.argv.find((a) => a.startsWith('--only=')) || '').slice(7);
const TOKEN = 'hooks-check-token-0123456789';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const freePort = () => new Promise((r) => { const s = net.createServer(); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => r(p)); }); });
const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { const v = await test().catch(() => false); if (v) return v; await sleep(500); }
  throw new Error('timed out waiting for ' + what);
};

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
if (spawnSync('where.exe', ['codex'], { encoding: 'utf8' }).status !== 0) die('codex is not on PATH');

// Which of the hooks in `codexHome` Codex says it runs, asked of Codex itself
// (`codex app-server`, `hooks/list`), as "event:trust" for each of ours
async function codexSays(env, codexHome) {
  const p = spawn('codex', ['app-server'], { env: { ...env, CODEX_HOME: codexHome }, shell: true });
  let buf = '';
  const waits = {};
  p.stdout.on('data', (d) => {
    buf += d;
    let i;
    while ((i = buf.indexOf('\n')) >= 0) {
      const l = buf.slice(0, i);
      buf = buf.slice(i + 1);
      try { const m = JSON.parse(l); if (m.id && waits[m.id]) waits[m.id](m); } catch { /* its own notices */ }
    }
  });
  let n = 0;
  const req = (method, params) => new Promise((r, j) => {
    const id = ++n;
    waits[id] = r;
    setTimeout(() => j(new Error(`codex app-server: no answer to ${method}`)), 20000);
    p.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
  });
  try {
    await req('initialize', { clientInfo: { name: 'check', version: '0' } });
    p.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'initialized' }) + '\n');
    const l = await req('hooks/list', { cwds: [path.dirname(codexHome)] });
    return (l.result?.data || []).flatMap((d) => d.hooks || [])
      .filter((h) => /--hook/.test(h.command || '') && /shikisha/i.test(h.command || ''))
      .map((h) => `${h.eventName}:${h.trustStatus}`);
  } finally {
    spawnSync('taskkill.exe', ['/PID', String(p.pid), '/T', '/F']);
  }
}

// The board in a headless Chrome at a phone's width: what the question says,
// a picture of it (target/shots/agent-hooks/phone.png), and its "no" pressed
async function onThePhone(url) {
  const chromeAt = [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA]
    .filter(Boolean).map((b) => path.join(b, 'Google', 'Chrome', 'Application', 'chrome.exe')).find((p) => fs.existsSync(p));
  if (!chromeAt) throw new Error('no Chrome to look with');
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'hooks-phone-'));
  const chrome = spawn(chromeAt, ['--headless=new', '--remote-debugging-port=9337', '--user-data-dir=' + profile, '--no-first-run', 'about:blank'], { stdio: 'ignore' });
  try {
    await sleep(1500);
    const { attach } = await import(new URL('../../.private/doc/proto/webrtc-http/cdp.mjs', import.meta.url).href);
    const cdp = await attach(9337);
    await cdp.call('Page.enable');
    await cdp.call('Runtime.enable');
    const js = async (expr) => {
      const r = await cdp.call('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true });
      return r.result && r.result.result && r.result.result.value;
    };
    await cdp.call('Emulation.setDeviceMetricsOverride', { width: 412, height: 915, deviceScaleFactor: 1, mobile: true });
    await cdp.call('Page.navigate', { url });
    await until(async () => js('!document.getElementById("sask").hidden'), 'the question on the phone', 30000);
    const seen = await js(`(() => { const b = document.getElementById("sask");
      return { title: b.querySelector(".vtitle").textContent, rows: b.querySelectorAll(".blist .brow2").length,
               no: b.querySelector(".quiet").textContent, go: b.querySelector(".go").textContent }; })()`);
    const shots = path.join(ROOT, 'target', 'shots', 'agent-hooks');
    fs.mkdirSync(shots, { recursive: true });
    const shot = await cdp.call('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(shots, 'phone.png'), Buffer.from(shot.result.data, 'base64'));
    await js('document.querySelector("#sask .quiet").click()');
    await sleep(1000);
    return seen;
  } finally {
    spawnSync('taskkill.exe', ['/PID', String(chrome.pid), '/T', '/F']);
  }
}

// One run: its own folder, home and app, started twice
async function trial(answer) {
  const RUN = path.join(os.tmpdir(), `sk-hooks-${answer}-${Date.now().toString(36)}`);
  const APP = path.join(RUN, 'app');
  const HOME = path.join(RUN, 'home');
  const CONFIG = path.join(APP, 'config', 'config.json');
  const LOG = path.join(APP, 'logs', 'hooks.log');
  const stopApp = () => ps('-Command',
    `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
    `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
    `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
  // A home where Codex and Claude Code have been used, and Gemini has not
  for (const d of [APP, path.join(HOME, '.codex'), path.join(HOME, '.claude'), path.join(RUN, 'localappdata'), path.join(RUN, 'work')]) {
    fs.mkdirSync(d, { recursive: true });
  }
  ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
  if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed');
  const port = await freePort();
  fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
  fs.writeFileSync(CONFIG, JSON.stringify({
    language: 'en',
    remote: { enabled: true, bind: '127.0.0.1', port, sticky_token: true, fixed_token: TOKEN },
    desks: [{ name: 'Only', id: 'only', folders: [{ cwd: path.join(RUN, 'work'), tabs: [{ name: 'shell', id: 'shell', command: ['cmd.exe'] }] }] }],
  }, null, 2));
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|CODEX_HOME)/i.test(k)));
  Object.assign(env, { LOCALAPPDATA: path.join(RUN, 'localappdata'), USERPROFILE: HOME, HOME });

  let cookies = '';
  const board = async () => {
    const r = await fetch(`http://127.0.0.1:${port}/?t=${TOKEN}`, { redirect: 'manual', signal: AbortSignal.timeout(10000) });
    cookies = r.headers.getSetCookie().map((c) => c.split(';')[0]).join('; ');
  };
  const state = async () => {
    const r = await fetch(`http://127.0.0.1:${port}/api/state?t=${TOKEN}`, { headers: { Cookie: cookies }, signal: AbortSignal.timeout(10000) });
    return r.json();
  };
  const intent = async (body) => {
    const r = await fetch(`http://127.0.0.1:${port}/api/intent?t=${TOKEN}`, { method: 'POST', body: JSON.stringify(body), headers: { Cookie: cookies }, signal: AbortSignal.timeout(10000) });
    return r.json();
  };
  const start = async () => {
    const child = spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' });
    child.unref();
    await until(async () => { await board(); return /(^|; )rs=/.test(cookies); }, 'the board', 60000);
    // Past the first frames, where the question is put up
    await until(async () => (await state()).ui !== undefined, 'the first state', 30000);
    await sleep(4000);
  };
  const log = () => (fs.existsSync(LOG) ? fs.readFileSync(LOG, 'utf8') : '');
  const read = (f) => (fs.existsSync(f) ? fs.readFileSync(f, 'utf8') : '');
  const codexHooks = path.join(HOME, '.codex', 'hooks.json');
  const claudeSettings = path.join(HOME, '.claude', 'settings.json');
  const checks = [];
  const check = (ok, what) => { checks.push(ok); console.log(`  ${ok ? 'ok  ' : 'FAIL'} ${what}`); };

  console.log(`--- answer: ${answer} (${RUN})`);
  try {
    await start();
    const asked = (await state()).ui?.hook_ask;
    check(!!asked, 'the first start asks');
    const names = (asked?.clis || []).map((c) => c.name);
    check(names.includes('Codex CLI') && names.includes('Claude Code') && !names.some((n) => /gemini/i.test(n)),
      `it asks about the CLIs used here, and only those: ${names.join(', ')}`);
    const codexRow = (asked?.clis || []).find((c) => c.name === 'Codex CLI');
    check(!!codexRow?.approval && codexRow.approval.toLowerCase().endsWith('config.toml'), 'it says where Codex keeps the approval');
    check(!!codexRow?.preview && codexRow.preview.includes('--hook'), 'it shows what gets written');
    if (answer === 'off') {
      // Answered the way a person on a phone answers it: the board opened at
      // a phone's width, the question read off it, its "no" pressed
      const seen = await onThePhone(`http://127.0.0.1:${port}/?t=${TOKEN}`);
      check(/report what they are doing/i.test(seen.title) && seen.rows >= 2,
        `the phone shows the question: "${seen.title}", ${seen.rows} CLIs, buttons "${seen.no}" / "${seen.go}"`);
    } else {
      const r = await intent({ kind: 'agenthooks', answer, seq: asked?.seq });
      check(!!r.ok, 'the answer is taken');
    }
    await until(async () => !(await state()).ui?.hook_ask, 'the question to go', 10000);
    check(true, 'the question goes once answered');
    if (answer === 'on') {
      await until(async () => /hooks: Codex CLI put right/.test(log()) || /could not be put right/.test(log()), 'the hooks to be put right', 60000);
      check(!/could not be put right/.test(log()), 'nothing failed' + (/could not be put right/.test(log()) ? `: ${log().split('\n').filter((l) => l.includes('could not be put right')).join(' | ')}` : ''));
      check(read(codexHooks).includes('--hook'), 'Codex hooks.json has the hook');
      check(read(claudeSettings).includes('--hook'), 'Claude Code settings.json has the hook');
      const toml = read(path.join(HOME, '.codex', 'config.toml'));
      check(/trusted_hash = "sha256:/.test(toml), 'Codex config.toml has the approval');
      const listed = await codexSays(env, path.join(HOME, '.codex'));
      check(listed.length > 0 && listed.every((h) => h.endsWith(':trusted')), `Codex runs them: ${listed.join(' ')}`);
    } else {
      check(!fs.existsSync(codexHooks) && !read(claudeSettings).includes('--hook'), 'nothing was written');
    }
    const saved = JSON.parse(read(CONFIG)).agent_hooks || {};
    check(answer === 'later' ? !saved['Codex CLI'] : saved['Codex CLI'] === answer, `the answer on record: ${JSON.stringify(saved)}`);

    // The second start
    stopApp();
    await sleep(1500);
    const before = log().length;
    await start();
    const again = (await state()).ui?.hook_ask;
    check(answer === 'later' ? !!again : !again, answer === 'later' ? 'the next start asks again' : 'the next start does not ask');
    if (answer === 'on') {
      await sleep(8000);
      check(!/put right|could not be put right/.test(log().slice(before)), 'the next start finds nothing to put right');
    }
  } catch (e) {
    checks.push(false);
    console.error('  stopped: ' + e.message);
  } finally {
    if (!process.argv.includes('--keep')) stopApp();
  }
  return checks.every(Boolean);
}

const answers = ONLY ? [ONLY] : ['on', 'off', 'later'];
let pass = 0;
for (const a of answers) if (await trial(a)) pass++;
console.log(`all: ${pass}/${answers.length}`);
process.exit(pass === answers.length ? 0 : 1);
