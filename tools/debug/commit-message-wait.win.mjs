/**
 * The git panel's "write the commit message" button comes back with the
 * settings saved while the AI is writing: the answer is read from the engine
 * the save replaced. (Ctrl+B a, automation off, is read by the window and
 * not the page, so it cannot be pressed from here; the engine's own test
 * covers it: a_snippet_answers_with_automation_stopped_...)
 *
 * The real ✨ button of a Git tab on a folder with a staged change is
 * pressed, the settings are written while the AI writes, and the message box
 * has to fill. The assistant AI is the one installed on this PC (COMMIT_AI,
 * by default claude): one short real question.
 *
 *     cargo build
 *     node tools/debug/commit-message-wait.win.mjs
 *
 * Nothing of a copy somebody is using is read, written or stopped; the copy
 * is stopped on the way out (COMMIT_KEEP=1 leaves it running to be looked at).
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-commit-wait');
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'local');
const REPO = path.join(RUN, 'repo');
const SHOTS = path.join(ROOT, 'target', 'shots', 'commit-message-wait');
const AI = process.env.COMMIT_AI || 'claude';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'SHIKISHA-TERM.exe' -or $_.Name -eq 'msedgewebview2.exe') -and ($_.CommandLine + '') -like '*sk-commit-wait*' } | ` +
  `ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);
const git = (...args) => {
  const r = spawnSync('git', ['-C', REPO, ...args], { encoding: 'utf8' });
  if (r.status !== 0) die('git ' + args.join(' ') + ': ' + r.stderr);
  return r.stdout;
};
const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, LOCAL, REPO, SHOTS, path.join(APP, 'config')]) fs.mkdirSync(d, { recursive: true });
git('init', '-q', '-b', 'main');
git('config', 'user.name', 'check'); git('config', 'user.email', 'check@example.invalid');
fs.writeFileSync(path.join(REPO, 'greet.py'), 'def greet(name):\n    return "hello " + name\n');
git('add', '.'); git('commit', '-q', '-m', 'first');
const change = (n) => {
  fs.writeFileSync(path.join(REPO, 'greet.py'),
    `def greet(name, times=${n}):\n    """Greet someone, as many times as asked"""\n    return " ".join(["hello " + name] * times)\n`);
  git('add', 'greet.py');
};

const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
const config = (note) => ({
  language: 'ja',
  remote: { enabled: false },
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  ai_engine: AI,
  desks: [{
    name: 'Check' + (note || ''), id: 'check',
    projects: [{ name: 'repo', at: REPO }],
    folders: [{ cwd: REPO, tabs: [{ name: 'Git', id: 'git', command: 'git' }] }],
  }],
});
const writeConfig = (note) => fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify(config(note), null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
const until = async (test, what, ms = 30000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(250); }
  throw new Error('timed out waiting for ' + what);
};
const portOf = () => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', 'shell', 'EBWebView', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return null;
  const n = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};

let ws;
try {
  writeConfig();
  try { fs.rmSync(path.join(LOCAL, 'ShikishaTerm', 'webview2', 'shell', 'EBWebView', 'DevToolsActivePort')); } catch (e) {}
  spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
  await until(() => portOf(), 'the board\'s DevTools port');
  let target;
  await until(async () => {
    const list = await (await fetch(`http://127.0.0.1:${portOf()}/json/list`)).json();
    target = list.find((t) => t.type === 'page' && /127\.0\.0\.1/.test(t.url));
    return !!target;
  }, 'the board page');
  const cdp = await connectCdp(target.webSocketDebuggerUrl);
  ws = cdp.ws;
  const { run, send } = cdp;
  const shot = async (name) => {
    const { data } = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, name + '.png'), Buffer.from(data, 'base64'));
  };
  // The Git tab in front, as a click on its row would put it
  await until(() => run('S.tabs.some(t => t.kind === "git")'), 'the Git tab', 30000);
  await run('send({kind:"select", tab: S.tabs.find(t => t.kind === "git").index}); true');
  await until(() => run('typeof G !== "undefined" && !!document.querySelector(".gai")'), 'the Git tab\'s ✨ button', 60000)
    .catch(async (e) => {
      console.log('   board: ' + await run('JSON.stringify({active: S.active, tabs: S.tabs.map(t => [t.index, t.name, t.kind, t.command])})'));
      await shot('0-no-button');
      throw e;
    });

  // Press ✨ and wait for the box to fill and the button to stop waiting
  const ask = async (name, meanwhile) => {
    await run('G.said = ""; gitSetMessage(""); true');
    const t0 = Date.now();
    await run('document.querySelector(".gai").click(); true');
    await until(() => run('G.busy === "message"'), 'the button to say the AI is writing', 5000);
    if (meanwhile) await meanwhile();
    // The AI's own deadline is three minutes; past that it is the bug. The
    // answer is the box filled or the panel saying why not -- the button's
    // own state is redrawn with the board and proves nothing alone
    const ok = await until(() => run('G.busy !== "message" && !!((gitUi && gitUi.msg.value) || G.said)'), 'the answer', 200000)
      .catch(() => false);
    const box = await run('(gitUi && gitUi.msg.value) || ""');
    const said = await run('G.said || ""');
    console.log(`   ${((Date.now() - t0) / 1000).toFixed(1)}s message: ${JSON.stringify(String(box).slice(0, 120))} said: ${JSON.stringify(said)}`);
    await shot(name);
    check(ok, name + ': the button stopped saying the AI is writing');
    check(String(box).trim().length > 0, name + ': the message box was filled');
  };

  console.log('1. the settings saved while the AI writes');
  change(3);
  await ask('1-settings-saved', async () => {
    await sleep(1500);
    writeConfig(' (saved)');
    await until(() => run('JSON.stringify(S).includes("Check (saved)")'), 'the save to be read', 15000)
      .then(() => console.log('   the settings were read again while the AI was writing'))
      .catch(() => check(false, 'the save was read while the AI was writing'));
  });
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  try { ws && ws.close(); } catch (e) {}
  if (!process.env.COMMIT_KEEP) stopApp();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
