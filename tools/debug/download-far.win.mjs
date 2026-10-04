/**
 * Where a file goes when a page of a folder on another machine saves it.
 *
 * A folder on an SSH server, with a small web server started in it over
 * there, and a browser tab of that folder on its port (carried to this PC the
 * way a port of a folder far away always is). The page's download link is
 * pressed through the page's debugging port, and what came of it is read off
 * the board and off both machines.
 *
 *     cargo build
 *     node tools/debug/download-far.win.mjs
 *
 * `--microvm` runs the same checks on a MicroVM instead: a sandbox made for
 * the run with E2B_API_TOKEN, killed on the way out (it costs a few minutes of
 * the account's time).
 *
 * Another server can be named in SSH_FAR_HOST, SSH_FAR_PORT, SSH_FAR_USER and
 * SSH_FAR_KEY (the OpenSSH that sftp-server.wsl.sh starts in WSL).
 *
 * Checked:
 *   there     the file goes to the server's ~/Downloads, under its own name,
 *             and no copy stays in this PC's Downloads folder; the list names
 *             the server and the path there
 *   twice     a second one of the same name is numbered on the server
 *   handed    a phone's "Save to this device" is handed the file, fetched from
 *             the server for the moment it takes
 *
 * Needs the
 * server ssh-far.win.mjs uses (.private/.env SSH_TEST_*). Everything it puts on
 * the server and in this PC's Downloads folder is removed on the way out.
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';
import { spawn, spawnSync } from 'node:child_process';
import net from 'node:net';
import {findChrome} from './chrome.mjs';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const RUN = path.join(os.tmpdir(), 'sk-download-far-' + process.pid);
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const SECRETS = path.join(APP, 'config', 'secrets.json');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
const until = async (test, what, ms = 30000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(400); }
  throw new Error('timed out waiting for ' + what);
};

const MAIN = path.dirname(spawnSync('git', ['-C', ROOT, 'rev-parse', '--path-format=absolute', '--git-common-dir'], { encoding: 'utf8' }).stdout.trim());
const dotenv = Object.fromEntries(fs.readFileSync(path.join(MAIN, '.private', '.env'), 'utf8')
  .split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#') && l.includes('='))
  .map((l) => [l.slice(0, l.indexOf('=')).trim(), l.slice(l.indexOf('=') + 1).trim().replace(/^"|"$/g, '')]));
// Another server named in SSH_FAR_*, as ssh-far.win.mjs reads it: the OpenSSH
// that sftp-server.wsl.sh starts in WSL, when the shared one is out of reach
const far = process.env.SSH_FAR_HOST ? {
  host: process.env.SSH_FAR_HOST, port: process.env.SSH_FAR_PORT, user: process.env.SSH_FAR_USER, key: process.env.SSH_FAR_KEY,
} : { host: dotenv.SSH_TEST_HOST, port: dotenv.SSH_TEST_PORT, user: dotenv.SSH_TEST_USER, key: dotenv.SSH_TEST_KEY, password: dotenv.SSH_TEST_PASSWORD };
const HOST = far.host, PORT = Number(far.port || 22), USER = far.user;
const PASSWORD = far.password, KEY = far.key;
const VM = process.argv.includes('--microvm');
const VM_KEY = dotenv.E2B_API_TOKEN;
if (VM && !VM_KEY) die('E2B_API_TOKEN is needed in .private/.env');
if (!VM && (!HOST || !USER || !(PASSWORD || KEY))) die('SSH_TEST_HOST, SSH_TEST_USER and a password or key are needed in .private/.env');
const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

const sdk = path.join(ROOT, 'target', 'ssh2-sdk');
if (!fs.existsSync(path.join(sdk, 'node_modules', 'ssh2'))) {
  fs.mkdirSync(sdk, { recursive: true });
  spawnSync('npm', ['init', '-y'], { cwd: sdk, shell: true });
  spawnSync('npm', ['i', 'ssh2', '--silent'], { cwd: sdk, shell: true });
}
const { Client } = createRequire(path.join(sdk, 'package.json'))('ssh2');
// On a MicroVM, the sandbox made for this run, asked through the service's SDK
let sandbox = null;
if (VM) {
  const dir = path.join(ROOT, 'target', 'e2b-sdk');
  const entry = path.join(dir, 'node_modules', 'e2b', 'dist', 'index.mjs');
  if (!fs.existsSync(entry)) {
    fs.mkdirSync(dir, { recursive: true });
    spawnSync('npm', ['init', '-y'], { cwd: dir, shell: true });
    spawnSync('npm', ['i', 'e2b', '--silent'], { cwd: dir, shell: true });
  }
  const { Sandbox } = await import('file://' + entry.replace(/\\/g, '/'));
  sandbox = await Sandbox.create('base', { apiKey: VM_KEY, timeoutMs: 15 * 60 * 1000, metadata: { shikisha: 'download-check' } });
  console.log('a MicroVM for this run: ' + sandbox.sandboxId);
}
const there = (cmd) => VM
  ? sandbox.commands.run(cmd, { timeoutMs: 60000 }).then((x) => x.stdout + x.stderr, (e) => String(e.result ? e.result.stdout + e.result.stderr : e))
  : new Promise((resolve) => {
  const c = new Client();
  let out = '';
  c.on('ready', () => c.exec(cmd, (err, st) => {
    if (err) { c.end(); resolve('ERR ' + err.message); return; }
    st.on('data', (d) => { out += d; }).stderr.on('data', (d) => { out += d; });
    st.on('close', () => { c.end(); resolve(out); });
  })).on('error', (e) => resolve('ERR ' + e.message))
    .connect({ host: HOST, port: PORT, username: USER, password: PASSWORD || undefined,
      privateKey: KEY ? fs.readFileSync(KEY) : undefined, readyTimeout: 20000 });
});

const MARK = 'shikisha-dl-check-' + process.pid;
const FILE = MARK + '.txt';
const HOME = (await there('printf %s "$HOME"')).trim();
if (!HOME.startsWith('/')) die('the server did not answer: ' + HOME);
const DIR = `${HOME}/${MARK}`;
const WEB = 18000 + (process.pid % 1000);
const cleanThere = () => there(`pkill -f "http.server ${WEB}" 2>/dev/null; rm -rf ${DIR} "$HOME/Downloads/${MARK}"*; true`);

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA|E2B)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
const portOf = (envName) => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', envName, 'EBWebView', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return null;
  const n = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};
const targetsOf = async (port) => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json());

const pcDownloads = path.join(os.homedir(), 'Downloads');
const freePort = () => new Promise((res) => {
  const srv = net.createServer().listen(0, '127.0.0.1', () => { const p = srv.address().port; srv.close(() => res(p)); });
});
const PHONE_PORT = await freePort();
const PHONE_KEY = 'downloadfarphone01234567';
let chrome = null;
try {
  console.log('the server: ' + (await there('uname -sr')).trim());
  await cleanThere();
  const page = `<!doctype html><meta charset=utf-8><title>far</title><a id=dl href="${FILE}" download>save</a>`;
  await there(`mkdir -p ${DIR}/proj && cd ${DIR}/proj && printf %s ${Buffer.from(page).toString('base64')} | base64 -d > index.html`
    + ` && printf 'made on the server\\n' > ${FILE}`
    + (VM ? '' : ` && (nohup python3 -m http.server ${WEB} --bind 127.0.0.1 > ${DIR}/web.log 2>&1 &) ; sleep 1`) + '; echo started');
  if (VM) await sandbox.commands.run(`cd ${DIR}/proj && python3 -m http.server ${WEB} --bind 127.0.0.1`, { background: true });
  await until(async () => (await there(`curl -s http://127.0.0.1:${WEB}/ | head -c 40`)).includes('doctype'), 'the web server over there', 15000);

  stopApp();
  fs.rmSync(RUN, { recursive: true, force: true });
  for (const d of [APP, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
  const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
  if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
  fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
  fs.writeFileSync(CONFIG, JSON.stringify({
    language: 'ja',
    remote: { enabled: true, bind: '127.0.0.1', port: PHONE_PORT, sticky_token: true, fixed_token: PHONE_KEY },
    resident: false,
    agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
    hosts: VM ? [{ name: 'srv', kind: 'e2b', template: 'base', minutes: 15 }]
      : [{ name: 'srv', at: `ssh://${USER}@${HOST}:${PORT}`, ...(KEY ? { key: KEY } : {}) }],
    desks: [{ name: 'Check', id: 'check', folders: [{
      cwd: `${DIR}/proj`, host: 'srv', ...(VM ? { sandbox: sandbox.sandboxId } : {}),
      tabs: [{ name: 'web', id: 'web', command: `browser far://${WEB}`,
        nav: { back: true, forward: true, reload: true, url: true, develop: true, point: true } }],
    }] }],
  }, null, 2));
  fs.writeFileSync(SECRETS, JSON.stringify({ tokens: VM ? { e2b_api_key: VM_KEY } : PASSWORD ? { 'ssh/host/srv/password': PASSWORD } : {} }, null, 2));
  spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

  let boardTarget;
  await until(async () => { const p = portOf('shell'); return p && (boardTarget = (await targetsOf(p)).find((t) => t.type === 'page')); }, 'the window\'s page', 40000);
  const board = await connectCdp(boardTarget, { timeout: 8000 });
  await until(() => board.run('!!S && S.tabs.some(t => t.kind === "browser")'), 'the page tab', 40000);
  // A server this copy has never met asks for its key to be trusted, on the
  // board: answered here as a person would, with the fingerprint it showed
  if (!VM) {
    await until(() => board.run('(S.key_changes || []).length > 0'), "the question about the server's key", 20000).catch(() => {});
    const asked = await board.run('(S.key_changes || []).length');
    await board.run('(S.key_changes || []).forEach(c => send({kind:"hostkey", machine:c.machine, fingerprint:c.now, trust:true})); true');
    if (asked) await board.run('send({kind:"go", what:"reload"})');
  }
  const tab = await board.run('S.tabs.find(t => t.kind === "browser").index');
  await board.run(`send({kind:"select", tab:${tab}})`);
  let pageTarget;
  await until(async () => {
    const p = portOf(path.join('profiles', 'default'));
    return p && (pageTarget = (await targetsOf(p)).find((t) => t.type === 'page' && t.title === 'far'));
  }, 'the page from the server, carried here', 60000);
  console.log('  page: ' + pageTarget.url);
  const pg = await connectCdp(pageTarget, { timeout: 8000 });
  await until(() => pg.run('document.readyState === "complete" && !!document.getElementById("dl")'), 'the page loaded', 30000);
  const press = async () => {
    const at = JSON.parse(await pg.run('(() => { const r = document.getElementById("dl").getBoundingClientRect(); return JSON.stringify([r.left + 5, r.top + 5]); })()'));
    for (const type of ['mousePressed', 'mouseReleased']) {
      await pg.send('Input.dispatchMouseEvent', { type, x: at[0], y: at[1], button: 'left', clickCount: 1 });
    }
  };
  console.log('1. saved where the folder is');
  await press();
  await until(() => board.run('S.downloads.length > 0 && S.downloads[0].state === "sending"'), 'the line saying it is on its way', 15000)
    .then(() => check(true, 'the line says it is on its way to the server'))
    .catch(() => console.log('  (too quick to see it on its way)'));
  await until(() => board.run('S.downloads.length === 1 && S.downloads[0].state === "done" && !!S.downloads[0].machine'), 'the file on the server', 60000);
  const d = JSON.parse(await board.run('JSON.stringify(S.downloads[0])'));
  console.log('  the line: ' + JSON.stringify({ name: d.name, state: d.state, machine: d.machine, path: d.path }));
  check(d.machine === 'srv' && d.path === `${HOME}/Downloads/${FILE}`, 'the line names the server and the path there');
  check((await there(`cat "${d.path}"`)) === 'made on the server\n', 'the file is whole on the server');
  check(!fs.readdirSync(pcDownloads).some((n) => n.startsWith(MARK)), 'no copy stays in this PC\'s Downloads folder');
  await board.run('sideReveal("downloads")');
  await sleep(500);
  const shot = await board.send('Page.captureScreenshot', { format: 'png' });
  fs.writeFileSync(path.join(SHOTS, 'download-far.png'), Buffer.from(shot.data, 'base64'));

  console.log('2. a second one of the same name');
  await press();
  await until(() => board.run('S.downloads.length === 2 && S.downloads[0].state === "done" && !!S.downloads[0].machine'), 'the second on the server', 60000);
  const second = await board.run('S.downloads[0].path');
  check(second === `${HOME}/Downloads/${MARK} (1).txt`, 'numbered on the server: ' + second);

  console.log('3. handed to a phone');
  const phoneDir = path.join(RUN, 'phone');
  fs.mkdirSync(phoneDir, { recursive: true });
  chrome = spawn(findChrome(), ['--headless=new', '--remote-debugging-port=0', '--user-data-dir=' + phoneDir,
    '--no-first-run', '--no-default-browser-check', 'about:blank'], { stdio: 'ignore' });
  const portFile = path.join(phoneDir, 'DevToolsActivePort');
  await until(() => fs.existsSync(portFile) && Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]) > 0, 'the phone\'s Chrome');
  const pport = Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]);
  let pt;
  await until(async () => (pt = (await targetsOf(pport)).find((t) => t.type === 'page')), 'the phone\'s page');
  const phone = await connectCdp(pt, { timeout: 8000 });
  await phone.send('Page.navigate', { url: `http://127.0.0.1:${PHONE_PORT}/?t=${PHONE_KEY}` });
  await until(() => phone.run('!!S && (S.downloads || []).length === 2'), 'the list on the phone', 30000);
  const got = await phone.run(`fetch("api/download?id=" + encodeURIComponent(S.downloads[1].id)).then(async r => r.status + ":" + (await r.text()))`);
  check(got === '200:made on the server\n', 'the phone is handed the file from the server: ' + JSON.stringify(got));
} catch (e) {
  failures += 1;
  console.error('  FAIL ' + e.message);
} finally {
  if (chrome && chrome.exitCode === null) spawnSync('taskkill.exe', ['/PID', String(chrome.pid), '/T', '/F']);
  stopApp();
  await cleanThere();
  if (sandbox) await sandbox.kill().then(() => console.log('the MicroVM is gone'), (e) => console.log('the MicroVM was not killed: ' + e));
  for (const f of fs.existsSync(pcDownloads) ? fs.readdirSync(pcDownloads) : []) if (f.startsWith(MARK)) fs.rmSync(path.join(pcDownloads, f), { force: true });
}
console.log(failures ? `${failures} failed` : 'done');
process.exit(failures ? 1 : 0);
