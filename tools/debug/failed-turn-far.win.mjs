/**
 * A turn that failed, on another machine: an AI tab on a MicroVM whose turn
 * ends in an error is answered FAILED with the error, one its provider
 * refused is answered FAILED and refused, and one that answers is answered
 * DONE -- each read off the record kept there, through the bridge.
 *
 *     cargo build
 *     node tools/debug/failed-turn-far.win.mjs [--keep]
 *
 * The bridge program is taken from bridge/ beside the build (dist.list): put a
 * Linux build there first, e.g. with cargo zigbuild in WSL.
 *
 * The AI there is a stand-in under Codex's name (nothing is spent): for each
 * line it is given it files a turn in a record where Codex keeps its own, in
 * Codex's shapes, and ends it the way the line asks -- CASE=overloaded as a
 * completion carrying "Selected model is at capacity", CASE=refused as one
 * carrying a refusal, anything else with an answer.
 *
 * Needs Windows, Node, and E2B_API_TOKEN in .private/.env. The MicroVM is
 * deleted at the end (--keep leaves it and the copy).
 */
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const KEEP = process.argv.includes('--keep');
const RUN = path.join(os.tmpdir(), 'sk-failed-far');
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const SECRETS = path.join(APP, 'config', 'secrets.json');
const HOOKS_LOG = path.join(APP, 'logs', 'hooks.log');
const MARK = 'shikisha-failed-far-' + process.pid;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
const checks = [];
const check = (ok, what, seen) => {
  checks.push(!!ok);
  console.log(`  ${ok ? 'PASS' : 'FAIL'} ${what}${ok || seen === undefined ? '' : `\n       saw: ${typeof seen === 'string' ? seen : JSON.stringify(seen)}`}`);
};
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'SHIKISHA-TERM.exe' -or $_.Name -eq 'msedgewebview2.exe') -and ($_.CommandLine + '' + $_.ExecutablePath) -like '*${path.basename(RUN)}*' } | ` +
  `ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);
const until = async (test, what, ms = 30000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(700); }
  throw new Error('timed out waiting for ' + what);
};
const logSince = (from) => (fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8').split(/\r?\n/).slice(from) : []);

const MAIN = path.dirname(spawnSync('git', ['-C', ROOT, 'rev-parse', '--path-format=absolute', '--git-common-dir'], { encoding: 'utf8' }).stdout.trim());
const dotenv = Object.fromEntries(fs.readFileSync(path.join(MAIN, '.private', '.env'), 'utf8')
  .split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#') && l.includes('='))
  .map((l) => [l.slice(0, l.indexOf('=')).trim(), l.slice(l.indexOf('=') + 1).trim().replace(/^"|"$/g, '')]));
const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
if (!fs.existsSync(path.join(ROOT, 'bridge', 'shikisha-bridge-x86_64-linux'))) die('no Linux bridge in bridge/ -- build one first');

// The stand-in Codex: a record where Codex keeps one, named and shaped as
// Codex names and shapes it, one turn filed for each line it is given
const STAND_IN = `#!/bin/sh
# Asked what it takes, as the app asks a CLI before starting it: answered and done
case " $* " in *" --help "*) echo "usage: codex [--no-daemon]"; exit 0;; esac
id=$(cat /proc/sys/kernel/random/uuid)
day=$(date -u +%Y/%m/%d)
mkdir -p "$HOME/.codex/sessions/$day"
rec="$HOME/.codex/sessions/$day/rollout-$(date -u +%Y-%m-%dT%H-%M-%S)-$id.jsonl"
now() { date -u +%Y-%m-%dT%H:%M:%S.000Z; }
printf '{"timestamp":"%s","type":"session_meta","payload":{"session_id":"%s","id":"%s","timestamp":"%s","cwd":"%s","originator":"codex-tui","source":"cli"}}\\n' "$(now)" "$id" "$id" "$(now)" "$PWD" >> "$rec"
echo "stand-in codex"
# What Codex's own SessionStart hook tells the terminal on another machine:
# which conversation this is
hook=$(printf '{"session_id":"%s","hook_event_name":"SessionStart","cwd":"%s"}' "$id" "$PWD" | base64 -w0)
printf '\\033]7727;shikisha-hook;session;%s;%s\\007' "$(date +%s%3N)" "$hook"
printf '› '
while read -r line; do
  said=$(printf %s "$line" | tr -d '\\033\\r"\\\\' | sed 's/\\[20[01]~//g')
  printf '{"timestamp":"%s","type":"event_msg","payload":{"type":"task_started"}}\\n' "$(now)" >> "$rec"
  printf '{"timestamp":"%s","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"%s"}]}}\\n' "$(now)" "$said" >> "$rec"
  printf '• Working (0s • esc to interrupt)'
  sleep 3
  printf '\\r\\033[2K'
  case "$said" in
    *CASE=overloaded*)
      printf '{"timestamp":"%s","type":"event_msg","payload":{"type":"task_complete","last_agent_message":null,"error":{"message":"Selected model is at capacity. Please try a different model.","codex_error_info":"server_overloaded"}}}\\n' "$(now)" >> "$rec"
      echo "■ Selected model is at capacity. Please try a different model.";;
    *CASE=refused*)
      printf '{"timestamp":"%s","type":"event_msg","payload":{"type":"task_complete","last_agent_message":null,"error":{"message":"This content was flagged for possible cybersecurity risk.","codex_error_info":"cyber_policy"}}}\\n' "$(now)" >> "$rec"
      echo "This content can't be shown";;
    *)
      printf '{"timestamp":"%s","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"FAR-ANSWER-OK"}]}}\\n' "$(now)" >> "$rec"
      printf '{"timestamp":"%s","type":"event_msg","payload":{"type":"task_complete","last_agent_message":"FAR-ANSWER-OK","error":null}}\\n' "$(now)" >> "$rec"
      echo "FAR-ANSWER-OK";;
  esac
  printf '› '
done
`;

// ── The MicroVM ────────────────────────────────────────────────────────────
const KEY = dotenv.E2B_API_TOKEN;
if (!KEY) die('E2B_API_TOKEN is needed in .private/.env');
const sdkDir = path.join(ROOT, 'target', 'e2b-sdk');
const entry = path.join(sdkDir, 'node_modules', 'e2b', 'dist', 'index.mjs');
if (!fs.existsSync(entry)) {
  fs.mkdirSync(sdkDir, { recursive: true });
  spawnSync('npm', ['init', '-y'], { cwd: sdkDir, shell: true });
  spawnSync('npm', ['i', 'e2b', '--silent'], { cwd: sdkDir, shell: true });
}
const { Sandbox } = await import('file://' + entry.replace(/\\/g, '/'));
const made = await (await fetch('https://api.e2b.app/sandboxes', {
  method: 'POST', headers: { 'X-API-Key': KEY, 'Content-Type': 'application/json' },
  body: JSON.stringify({ templateID: 'base', timeout: 900, autoPause: true, autoResume: { enabled: true },
    metadata: { shikisha: '1', project: 'failed-turn-far-check' } }),
})).json();
if (!made.sandboxID) die('no machine: ' + JSON.stringify(made));
const box = await Sandbox.connect(made.sandboxID, { apiKey: KEY });
const there = async (cmd) => {
  const r = await box.commands.run(cmd, { timeoutMs: 60000 }).catch((e) => e.result || { stdout: '', stderr: String(e) });
  return r.stdout + r.stderr;
};
const HOME = '/home/user';
const DIR = `${HOME}/${MARK}`;
const cleanup = async () => {
  stopApp();
  if (KEEP) { console.log(`kept: ${RUN} and MicroVM ${box.sandboxId}`); return; }
  await fetch('https://api.e2b.app/sandboxes/' + box.sandboxId, { method: 'DELETE', headers: { 'X-API-Key': KEY } }).catch(() => {});
  fs.rmSync(RUN, { recursive: true, force: true });
};

// ── The copy here ──────────────────────────────────────────────────────────
let door;
const openDoor = async (pid) => {
  const tokenFile = path.join(APP, 'data', 'api-token');
  await until(() => fs.existsSync(tokenFile), 'the api token', 60000);
  const token = fs.readFileSync(tokenFile, 'utf8').trim();
  door = async (method, ...params) => {
    const sock = net.connect(`\\\\.\\pipe\\shikisha-${pid}`);
    await new Promise((r, j) => { sock.once('connect', r); sock.once('error', j); });
    let b = '';
    const lines = [];
    let wake = null;
    sock.on('data', (d) => {
      b += d.toString('utf8');
      let i;
      while ((i = b.indexOf('\n')) >= 0) { lines.push(JSON.parse(b.slice(0, i))); b = b.slice(i + 1); if (wake) wake(); }
    });
    const next = async () => { while (!lines.length) await new Promise((r) => { wake = r; }); return lines.shift(); };
    sock.write(JSON.stringify({ token }) + '\n');
    await next();
    sock.write(JSON.stringify({ id: '1', method, params }) + '\n');
    const a = await next();
    sock.end();
    if (!a.ok) throw new Error(`${method}: ${a.error}`);
    return a.result;
  };
};
const screen = (id) => door('tab_screen', id).then((s) => String(s || '')).catch(() => '');
const state = (id) => door('state', id).then(String).catch((e) => `(${e.message})`);

try {
  console.log('the other machine: ' + (await there('uname -srm')).trim());
  const b64 = Buffer.from(STAND_IN).toString('base64');
  await there(`mkdir -p ${DIR}/bin && printf %s ${b64} | base64 -d > ${DIR}/bin/codex && chmod +x ${DIR}/bin/codex`);
  stopApp();
  fs.rmSync(RUN, { recursive: true, force: true });
  for (const d of [APP, LOCAL]) fs.mkdirSync(d, { recursive: true });
  const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
  const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
  if (staged.status !== 0 || !fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);
  fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
  const host = { name: 'vm', kind: 'e2b', template: 'base', minutes: 15 };
  fs.writeFileSync(CONFIG, JSON.stringify({
    language: 'en', remote: { enabled: false }, resident: false, external_api: { access: 'user' },
    hosts: [host], bridges: ['vm'],
    desks: [{ name: 'Far', id: 'far', folders: [
      { cwd: DIR, host: 'vm', sandbox: box.sandboxId, tabs: [{ name: 'farcodex', id: 'farcodex', command: `${DIR}/bin/codex` }] },
    ] }],
  }, null, 2));
  fs.writeFileSync(SECRETS, JSON.stringify({ tokens: { e2b_api_key: KEY } }, null, 2));
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|E2B|CODEX)/i.test(k)));
  env.LOCALAPPDATA = LOCAL;
  const child = spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' });
  child.unref();
  await openDoor(child.pid);

  console.log('1. the stand-in Codex comes up on the MicroVM, with the bridge');
  // A MicroVM's terminal waits until something needs it: being asked for its screen does
  await until(async () => (await screen('farcodex')).includes('stand-in codex'), 'the far Codex to start', 240000)
    .catch(async () => console.log('       farcodex: ' + (await screen('farcodex')).slice(-300)));
  await until(() => logSince(0).some((l) => l.includes('bridge: connected to')), 'the bridge to connect', 180000).catch(() => {});
  check(logSince(0).some((l) => l.includes('bridge: connected to')), 'the bridge is connected');
  // Its conversation found, so what it says is read from its record
  await sleep(15000);

  const ask = async (what, text) => {
    const from = logSince(0).length;
    const t0 = Date.now();
    const v = await door('ask_tab', 'farcodex', what, text, { timeout_ms: 180000 });
    console.log(`       answered in ${Math.round((Date.now() - t0) / 1000)}s: ${JSON.stringify({ state: v.state, error: v.error, refused: v.refused, reply: v.state === 'DONE' ? v.reply : undefined, source: v.source })}`);
    return { v, log: logSince(from) };
  };

  console.log('2. a turn that ends in an error');
  const one = await ask('A check.', 'Reply with READY. CASE=overloaded');
  check(one.v.state === 'FAILED', 'answered FAILED', one.v);
  check(/at capacity/.test(one.v.error || ''), 'with what Codex said went wrong, from the record there', one.v);
  check(!one.v.refused, 'not taken for a refusal');
  const dot = await until(async () => (await state('farcodex')) === 'FAILED', 'the far tab to read failed', 30000).catch(() => false);
  check(dot, 'the far tab itself reads as failed', await state('farcodex'));
  check(!one.log.some((l) => /could not say how the turn ended/.test(l)), 'the turn\'s end came from the bridge, not the long way',
    one.log.filter((l) => /reader:/.test(l)).slice(-3).join(' | '));

  console.log('3. a request the provider refused');
  const two = await ask('Another check.', 'Reply with READY. CASE=refused');
  check(two.v.state === 'FAILED' && two.v.refused === true, 'answered FAILED, refused', two.v);
  check(/same words will be refused again/.test(two.v.note || ''), 'and says the same words will be refused again', two.v.note);

  console.log('4. a turn that answers');
  const three = await ask('Last check.', 'Reply with READY. CASE=answer');
  check(three.v.state === 'DONE' && /FAR-ANSWER-OK/.test(three.v.reply || ''), 'answered DONE with its words', three.v);
  check(three.v.source === 'record', 'read from the record there', three.v.source);

  console.log('5. the bridge there says how each of the three turns itself ended');
  // Asked as this PC asks it: where each turn begins, and the end after it
  const bridgeBin = (await there(`ls -1 ${HOME}/.local/share/shikisha/bridge/shikisha-bridge* 2>/dev/null | head -n 1`)).trim();
  const rec = (await there(`ls -1t ${HOME}/.codex/sessions/*/*/*/rollout-*.jsonl | head -n 1`)).trim();
  const id = (/rollout-.*-([0-9a-f-]{36})\.jsonl$/.exec(rec) || [])[1];
  const starts = (await there(`grep -b '"role":"user"' ${rec} | cut -d: -f1`)).trim().split(/\s+/).map(Number);
  const asks = starts.map((from, i) => JSON.stringify({ t: 'op', id: i + 1, op: 'turn_end', p: { glob: `${HOME}/.codex/sessions/*/*/*/rollout-*-{id}.jsonl`, id, from } }));
  const said = await there(`printf '%s\\n' ${asks.map((a) => `'${a}'`).join(' ')} | ${bridgeBin} serve`);
  const answers = said.split('\n').filter((l) => l.includes('"id"')).map((l) => { try { return JSON.parse(l); } catch { return null; } }).filter((v) => v && v.id);
  const ended = answers.sort((a, b) => a.id - b.id).map((a) => a.r || a.result || a.ok || a);
  const kinds = ended.map((e) => (e && e.failed ? e.kind : e && e.failed === false ? 'answer' : JSON.stringify(e)));
  check(starts.length === 3 && JSON.stringify(kinds) === JSON.stringify(['error', 'refused', 'answer']),
    'each turn\'s own end: the error, the refusal, the answer', { bridge: bridgeBin, starts, said: said.slice(0, 600) });
} catch (e) {
  console.error(e.stack || e.message);
  checks.push(false);
} finally {
  const ok = checks.length > 0 && checks.every(Boolean);
  console.log(`\n${ok ? 'all passed' : 'FAILED'} (${checks.filter(Boolean).length}/${checks.length})`);
  await cleanup();
  process.exit(ok ? 0 : 1);
}
