/**
 * An AI tab in a folder on a MicroVM comes back, after the app is restarted,
 * running its CLI on the conversation it was having -- through the running
 * app and a real E2B account.
 *
 * The machine is given a stand-in `claude` that writes down the arguments it
 * was started with and makes the record of its conversation where Claude Code
 * keeps one, so what the app typed there is read back from the machine:
 *
 *   1. the first start hands it a new conversation under an id the app chose
 *   2. the app writes that id down
 *   3. restarted, the tab resumes that conversation, on the machine
 *   4. restarted with the record gone from the machine, it starts a new one
 *      under the same id, rather than failing on an id nobody has
 *   5. quitting the app ends its shell on the machine, and the AI in it, so
 *      the next start does not put a second one on the same conversation
 *
 *     cargo build
 *     node tools/debug/microvm-resume.win.mjs
 *
 * Needs Windows, Node, and E2B_API_TOKEN in .private/.env. Makes one machine
 * for a few minutes and deletes it on the way out.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-microvm-resume');
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const SECRETS = path.join(APP, 'config', 'secrets.json');
const FOLDER = '/home/user/proj';
const CALLS = '/home/user/claude-calls.log';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
const until = async (test, what, ms = 60000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(500); }
  throw new Error('timed out waiting for ' + what);
};

const MAIN = path.dirname(spawnSync('git', ['-C', ROOT, 'rev-parse', '--path-format=absolute', '--git-common-dir'], { encoding: 'utf8' }).stdout.trim());
const dotenv = Object.fromEntries(fs.readFileSync(path.join(MAIN, '.private', '.env'), 'utf8')
  .split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#') && l.includes('='))
  .map((l) => [l.slice(0, l.indexOf('=')).trim(), l.slice(l.indexOf('=') + 1).trim().replace(/^"|"$/g, '')]));
const KEY = dotenv.E2B_API_TOKEN;
if (!KEY) die('E2B_API_TOKEN is needed in .private/.env');
const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

function pathToSdk() {
  const dir = path.join(ROOT, 'target', 'e2b-sdk');
  const entry = path.join(dir, 'node_modules', 'e2b', 'dist', 'index.mjs');
  if (!fs.existsSync(entry)) {
    fs.mkdirSync(dir, { recursive: true });
    spawnSync('npm', ['init', '-y'], { cwd: dir, shell: true });
    spawnSync('npm', ['i', 'e2b', '--silent'], { cwd: dir, shell: true });
  }
  return 'file://' + entry.replace(/\\/g, '/');
}

// Claude Code's own arguments, and where it keeps a conversation's record
const STAND_IN = `#!/bin/sh
echo "$*" >> ${CALLS}
id=""
while [ $# -gt 0 ]; do case "$1" in --session-id|--resume) id="$2"; shift;; esac; shift; done
mkdir -p "$HOME/.claude/projects/-home-user-proj"
touch "$HOME/.claude/projects/-home-user-proj/$id.jsonl"
echo "stand-in claude on $id"
exec cat
`;

const { Sandbox } = await import(pathToSdk());
console.log('making a MicroVM');
const box = await Sandbox.create('base', { apiKey: KEY, timeoutMs: 10 * 60 * 1000, metadata: { shikisha: '1', project: 'resume-check' } });
const inside = async (cmd) => {
  const r = await box.commands.run(cmd, { timeoutMs: 60000 }).catch((e) => e.result || { stdout: '', stderr: String(e) });
  return (r.stdout + r.stderr).trim();
};
const calls = async () => (await inside(`cat ${CALLS} 2>/dev/null`)).split('\n').filter(Boolean);
const remembered = () => {
  const f = [path.join(APP, 'last-session'), path.join(APP, 'data', 'last-session')].find((p) => fs.existsSync(p));
  if (!f) return null;
  const s = JSON.parse(fs.readFileSync(f, 'utf8'));
  return (s.desks || []).flatMap((d) => d.tabs || []).find((t) => t.id === 'ai') || null;
};
// The window's page, to press its own close button
const boardOf = async () => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', 'shell', 'EBWebView', 'DevToolsActivePort');
  let target;
  await until(async () => {
    if (!fs.existsSync(f)) return false;
    const port = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
    target = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === 'page');
    return !!target;
  }, 'the window\'s page', 30000);
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r, { once: true }));
  return (expression) => ws.send(JSON.stringify({ id: 1, method: 'Runtime.evaluate', params: { expression } }));
};
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|E2B)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
const start = () => spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

try {
  await box.files.write('/tmp/claude', STAND_IN);
  await inside('sudo install -m 755 /tmp/claude /usr/local/bin/claude && mkdir -p ' + FOLDER);

  stopApp();
  await sleep(800);
  fs.rmSync(RUN, { recursive: true, force: true });
  for (const d of [APP, LOCAL]) fs.mkdirSync(d, { recursive: true });
  const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
  if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
  fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
  fs.writeFileSync(CONFIG, JSON.stringify({
    language: 'ja',
    remote: { enabled: false },
    // Closing the window quits, rather than going to the notification area
    resident: false,
    hosts: [{ name: 'vm', kind: 'e2b', template: 'base', minutes: 10 }],
    desks: [{ name: 'Check', id: 'check', folders: [{
      cwd: FOLDER, host: 'vm', sandbox: box.sandboxId,
      tabs: [{ name: 'ai', id: 'ai', command: 'claude' }],
    }] }],
  }, null, 2));
  fs.writeFileSync(SECRETS, JSON.stringify({ tokens: { e2b_api_key: KEY } }, null, 2));

  console.log('1. the first start: a new conversation under the app\'s id');
  start();
  await until(async () => (await calls()).length >= 1, 'the CLI to start on the machine', 90000);
  const first = (await calls())[0];
  const minted = (first.match(/^--session-id ([0-9a-f-]{36})$/) || [])[1];
  check(!!minted, 'started with a conversation id: ' + first);

  console.log('2. the app writes that id down');
  await until(() => remembered()?.session === minted, 'the id in the last session', 20000)
    .then(() => check(true, 'remembered as the tab\'s conversation'))
    .catch(() => check(false, 'remembered as the tab\'s conversation: ' + JSON.stringify(remembered())));

  console.log('3. restarted, it resumes that conversation there');
  stopApp();
  await sleep(1500);
  start();
  await until(async () => (await calls()).length >= 2, 'the CLI to start again', 90000);
  check((await calls())[1] === '--resume ' + minted, 'resumed: ' + (await calls())[1]);

  console.log('4. restarted with the record gone, a new one under the same id');
  await sleep(4000);
  stopApp();
  await inside('rm -f $HOME/.claude/projects/*/' + minted + '.jsonl');
  await sleep(1500);
  start();
  await until(async () => (await calls()).length >= 3, 'the CLI to start a third time', 90000);
  check((await calls())[2] === '--session-id ' + minted, 'a new conversation under the same id: ' + (await calls())[2]);

  console.log('5. quitting ends its shell on the machine');
  const running = ps('-Command', `(Get-Process -Name 'SHIKISHA-TERM' | Where-Object { $_.Path -like '${RUN}\*' } | Select-Object -First 1).Id`).stdout.trim();
  const ours = async () => (await box.commands.list()).filter((p) => (p.tag || '').startsWith('shikisha-' + running + '-'));
  check((await ours()).length === 1, 'its shell is there while it runs');
  const press = await boardOf();
  // A tab at work makes quitting ask first, in a box of its own: waited out
  await sleep(12000);
  await until(async () => { press('typeof winAct === "function" && winAct("close")'); return true; }, 'the close button');
  await until(() => !ps('-Command', `Get-Process -Id ${running} -ErrorAction SilentlyContinue`).stdout.trim(), 'the app to quit', 30000);
  check((await ours()).length === 0, 'and gone once it quit: ' + JSON.stringify(await ours()));
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + (e.stack || e));
  console.log('    (on the machine: ' + JSON.stringify((await box.commands.list()).map((p) => ({ pid: p.pid, tag: p.tag, cmd: p.cmd, args: p.args }))) + ')');
} finally {
  stopApp();
  await box.kill().catch(() => {});
  console.log('machine deleted');
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
