/**
 * The editor and the column's file list on a folder that is on a MicroVM,
 * through the running app's own window and a real E2B account.
 *
 * What is checked, in order:
 *
 *   1. the file list shows the machine's folder, and a folder in it opens out
 *   2. a search by name and by what is inside runs on the machine
 *   3. a file opens in the editor from the machine, and saves back to it
 *   4. a Shift_JIS file is read as that and saved back byte for byte
 *   5. a file the machine changes under a clean editor is followed, while a
 *      terminal on that machine is moving
 *   6. a save over a file that changed since it was read is refused
 *   7. a path outside the folder is refused without asking the machine
 *
 * Every write is checked against the machine itself, not only the page.
 *
 *     cargo build
 *     node tools/debug/microvm-editor.win.mjs
 *
 * Needs Windows, Node, and E2B_API_TOKEN in .private/.env. Makes one machine
 * for a few minutes and deletes it on the way out. Isolated the way
 * microvm-flow.win.mjs is.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const RUN = path.join(os.tmpdir(), 'sk-microvm-editor');
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const SECRETS = path.join(APP, 'config', 'secrets.json');
const FOLDER = '/home/user/proj';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);

const MAIN = path.dirname(spawnSync('git', ['-C', ROOT, 'rev-parse', '--path-format=absolute', '--git-common-dir'], { encoding: 'utf8' }).stdout.trim());
const dotenv = Object.fromEntries(fs.readFileSync(path.join(MAIN, '.private', '.env'), 'utf8')
  .split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#') && l.includes('='))
  .map((l) => [l.slice(0, l.indexOf('=')).trim(), l.slice(l.indexOf('=') + 1).trim().replace(/^"|"$/g, '')]));
const KEY = dotenv.E2B_API_TOKEN;
if (!KEY) die('E2B_API_TOKEN is needed in .private/.env');

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

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

// The machine, and what is in its folder. Shift_JIS for 氏名,メモ / 山田,済
const { Sandbox } = await import(pathToSdk());
console.log('making a MicroVM');
const box = await Sandbox.create('base', { apiKey: KEY, timeoutMs: 10 * 60 * 1000, metadata: { shikisha: '1', project: 'editor-check' } });
const SJIS = Buffer.from([0x8e, 0x81, 0x96, 0xbc, 0x2c, 0x83, 0x81, 0x83, 0x82, 0x0d, 0x0a, 0x8e, 0x52, 0x93, 0x63, 0x2c, 0x8d, 0xcf, 0x0d, 0x0a]);
const inside = async (cmd) => {
  const r = await box.commands.run(cmd, { timeoutMs: 60000 }).catch((e) => e.result || { stdout: '', stderr: String(e) });
  return (r.stdout + r.stderr).trim();
};
const bytesThere = async (p) => Buffer.from(await box.files.read(p, { format: 'bytes' }));

try {
  await box.files.write(FOLDER + '/readme.md', '# proj\nhello NEEDLE here\n');
  await box.files.write(FOLDER + '/src/main.rs', 'fn main() {}\n');
  await box.files.write(FOLDER + '/sjis.csv', SJIS.buffer.slice(SJIS.byteOffset, SJIS.byteOffset + SJIS.length));
  await inside(`cd ${FOLDER} && git init -q && git add -A && git -c user.email=a@b -c user.name=a commit -qm init`);

  console.log('starting this checkout\'s build, isolated');
  stopApp();
  await sleep(800);
  fs.rmSync(RUN, { recursive: true, force: true });
  for (const d of [APP, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
  const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
  if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
  fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
  fs.writeFileSync(CONFIG, JSON.stringify({
    language: 'ja',
    remote: { enabled: false },
    hosts: [{ name: 'vm', kind: 'e2b', template: 'base', minutes: 10 }],
    desks: [{
      name: 'Check', id: 'check', folders: [{
        cwd: FOLDER, host: 'vm', sandbox: box.sandboxId,
        // A terminal there that keeps changing: what says the machine is
        // awake and somebody is working in it
        tabs: [{ name: 'sh', id: 'sh', command: 'top -d 1' }, { name: 'ed', id: 'ed', command: 'editor' }],
      }],
    }],
  }, null, 2));
  fs.writeFileSync(SECRETS, JSON.stringify({ tokens: { e2b_api_key: KEY } }, null, 2));

  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|E2B)/i.test(k)));
  env.LOCALAPPDATA = LOCAL;
  env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
  spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

  const portOf = (envName) => {
    const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', envName, 'EBWebView', 'DevToolsActivePort');
    if (!fs.existsSync(f)) return null;
    const n = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
    return Number.isFinite(n) && n > 0 ? n : null;
  };
  const targetsOf = async (port) => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json());
  const until = async (test, what, ms = 20000) => {
    const end = Date.now() + ms;
    while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(250); }
    throw new Error('timed out waiting for ' + what);
  };
  let target;
  await until(async () => {
    const p = portOf('shell');
    if (!p) return false;
    target = (await targetsOf(p)).find((t) => t.type === 'page');
    return !!target;
  }, 'the window\'s page', 40000);
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r, { once: true }));
  let id = 0;
  const waiting = new Map();
  ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data);
    if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
  });
  const cdp = (method, params = {}) => new Promise((res, rej) => {
    const n = ++id;
    waiting.set(n, (m) => (m.error ? rej(new Error(method + ': ' + JSON.stringify(m.error))) : res(m.result)));
    ws.send(JSON.stringify({ id: n, method, params }));
  });
  const run = async (expression) => {
    const r = await cdp('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  };
  const shot = async (label) => {
    const r = await cdp('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, `microvm-editor-${label}.png`), Buffer.from(r.data, 'base64'));
  };

  await until(() => run('!!(S && S.tabs && S.tabs.some(t => t.id === "sh"))'), 'the board with the folder\'s tabs', 40000);
  // Every answer the file door gives, kept for the checks
  await run(`(() => { window.__heard = []; const o = window.__files; window.__files = d => { window.__heard.push(d); return o(d); }; return true; })()`);
  const ask = (panel, act, args) => run(`(send({kind: "files", panel: ${JSON.stringify(panel)}, act: ${JSON.stringify(act)}, args: ${JSON.stringify(args)}}), true)`);
  const heard = (test) => run(`(window.__heard || []).filter(${test}).pop() || null`);
  const answer = async (test, what, ms = 60000) => { let d; await until(async () => (d = await heard(test)), what, ms); return d; };

  console.log('1. the file list shows the machine\'s folder');
  await ask('sh', 'ls', { at: '' });
  let d = await answer('d => d.act === "ls" && d.at === ""', 'the folder listed');
  const names = (d.rows || []).map((r) => r.name);
  check(d.ok && names.includes('readme.md') && names.includes('src') && !names.includes('.git'), 'the folder there, git\'s own left out: ' + names.join(', '));
  await ask('sh', 'ls', { at: 'src' });
  d = await answer('d => d.act === "ls" && d.at === "src"', 'a folder in it');
  check(d.ok && (d.rows || []).some((r) => r.name === 'main.rs'), 'a folder in it opens out');

  console.log('2. search runs on the machine');
  await ask('sh', 'find', { q: 'main' });
  d = await answer('d => d.act === "find" && d.q === "main"', 'a search by name');
  check(d.ok && (d.hits || []).map((h) => h.path).join() === 'src/main.rs', 'by name: ' + JSON.stringify(d.hits));
  await ask('sh', 'grep', { q: 'needle' });
  d = await answer('d => d.act === "grep" && d.q === "needle"', 'a search by contents');
  check(d.ok && d.hits?.length === 1 && d.hits[0].path === 'readme.md' && d.hits[0].line === 2 && /NEEDLE/.test(d.hits[0].text), 'by contents, with the line: ' + JSON.stringify(d.hits));
  await ask('sh', 'grep', { q: '$(touch /tmp/pwned)' });
  await answer('d => d.act === "grep" && d.q === "$(touch /tmp/pwned)"', 'a search with a command in it');
  check((await inside('test -e /tmp/pwned && echo yes || echo no')) === 'no', 'what is typed in the search box stays text on the machine');

  console.log('3. a file opens in the editor from the machine, and saves back');
  await run('(send({kind: "editopen", panel: "sh", path: "readme.md"}), true)');
  await until(() => run('ED.path === "readme.md" && !ED.loading && !!edAce && edAce.getValue().includes("NEEDLE")'), 'the file in the editor', 60000);
  check(true, 'readme.md opened with the machine\'s contents');
  await shot('1-open');
  await run('edAce.setValue("# proj\\nhello from the editor\\n", 1); true');
  await until(() => run('ED.dirty'), 'the editor to know it was typed in');
  await run('editSave(); true');
  await until(() => run('!ED.dirty && !ED.bad'), 'the save', 60000);
  check((await bytesThere(FOLDER + '/readme.md')).toString() === '# proj\nhello from the editor\n', 'the machine has what was saved');

  console.log('4. a Shift_JIS file round trip');
  await run('(send({kind: "editopen", panel: "sh", path: "sjis.csv"}), true)');
  await until(() => run('ED.path === "sjis.csv" && !ED.loading && !!edAce && edAce.getValue().includes("山田")'), 'the Shift_JIS file', 60000);
  check(await run('ED.encoding') === 'Shift_JIS', 'read as Shift_JIS: ' + await run('ED.encoding'));
  await run('edAce.setValue(edAce.getValue().replace("済", "未"), 1); true');
  await until(() => run('ED.dirty'), 'typed');
  await run('editSave(); true');
  await until(() => run('!ED.dirty && !ED.bad'), 'the save', 60000);
  const want = Buffer.from(SJIS); want[16] = 0x96; want[17] = 0xa2; // 未
  const got = await bytesThere(FOLDER + '/sjis.csv');
  check(got.equals(want), 'saved back in Shift_JIS, byte for byte: ' + got.toString('hex'));

  console.log('5. a change on the machine is followed by a clean editor');
  await box.files.write(FOLDER + '/sjis.csv', Buffer.concat([want, Buffer.from('x,y\r\n')]));
  await until(() => run('edAce.getValue().includes("x,y")'), 'the editor to follow the file', 30000)
    .then(() => check(true, 'the editor followed what the machine wrote'))
    .catch(() => check(false, 'the editor followed what the machine wrote'));

  console.log('6. a save over a file that changed since it was read is refused');
  await run('edAce.setValue(edAce.getValue() + "mine,1\\r\\n", 1); true');
  await until(() => run('ED.dirty'), 'typed');
  await box.files.write(FOLDER + '/sjis.csv', Buffer.from('somebody else\r\n'));
  await sleep(500);
  await run('editWrite({}); true');
  await until(() => run('ED.bad || !!ED.outside'), 'the refusal', 60000);
  await until(() => run('ED.bad'), 'the refusal', 60000).catch(() => {});
  check((await bytesThere(FOLDER + '/sjis.csv')).toString() === 'somebody else\r\n', 'the other write is still there');
  check(await run('ED.bad && /変わって/.test(ED.said)'), 'and the editor says why: ' + await run('ED.said'));
  await shot('2-refused');

  console.log('7. outside the folder is refused at once');
  await ask('ed', 'read', { path: '../../../etc/passwd' });
  d = await answer('d => d.act === "read" && d.ok === false && !d.path', 'the refusal', 5000);
  check(!!d && /外/.test(d.error || ''), 'refused: ' + (d && d.error));
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + (e.stack || e));
} finally {
  stopApp();
  await box.kill().catch(() => {});
  console.log('machine deleted');
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
