/**
 * The 🖥️ at the foot of the board's sidebar, in this PC's real window, with a
 * real server version run in WSL:
 *
 *   1. paired with one, the 🖥️ is there, and pressed it opens that board in a
 *      window of its own (a ticket asked for with the key this PC was handed)
 *   2. with a second one whose key is not in the secrets, the 🖥️ is a list;
 *      the one without a key says why, on the board, and opens nothing
 *
 *     cargo build
 *     node tools/debug/boards-button.win.mjs
 *
 * Needs WSL with cargo. Builds shikisha-server there, runs it with a folder of
 * its own and the board on 127.0.0.1:8798, and stops it, the copy here and
 * the window it opened on the way out. Nothing of a copy somebody is using is
 * read, written or stopped.
 */
import { connectCdp } from './chrome.mjs';
import { spawnSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const WSL_ROOT = '/mnt/' + ROOT[0].toLowerCase() + ROOT.slice(2).replace(/\\/g, '/');
const PORT = 8798;
const BOARD = `http://127.0.0.1:${PORT}`;
const SRV_HOME = '/tmp/sk-boards-button';
const RUN = path.join(os.tmpdir(), 'sk-boards-button');
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const HOOKS_LOG = path.join(APP, 'logs', 'hooks.log');
const wsl = (script) => spawnSync('wsl', ['-e', 'bash', '-c', script], { encoding: 'utf8', env: { ...process.env, MSYS_NO_PATHCONV: '1' } });
const ps = (...a) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...a], { encoding: 'utf8' });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const until = async (test, what, ms = 30000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(500); }
  throw new Error('timed out waiting for ' + what);
};
const log = () => (fs.existsSync(HOOKS_LOG) ? fs.readFileSync(HOOKS_LOG, 'utf8') : '');
// This copy's processes (not the server in WSL, whose folder is named alike),
// and the window it opened (`--connect` to this board), by their paths
const stopOurs = () => ps('-Command',
  `Get-CimInstance Win32_Process | Where-Object { (($_.Name -eq 'SHIKISHA-TERM.exe' -or $_.Name -eq 'msedgewebview2.exe') -and ($_.CommandLine + '' + $_.ExecutablePath) -like '*${path.basename(RUN)}*') -or ` +
  `($_.Name -eq 'SHIKISHA-TERM.exe' -and $_.CommandLine -like '*--connect*127.0.0.1:${PORT}*') } | ` +
  `ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) { console.error('no build at target\\debug -- run cargo build first'); process.exit(2); }

console.log('building shikisha-server in WSL');
const built = wsl(`cd ${WSL_ROOT} && CARGO_TARGET_DIR=$HOME/sk-target ~/.cargo/bin/cargo build -q -p shikisha-server 2>&1 | tail -3`);
if (built.status !== 0) { console.error(built.stdout + built.stderr); process.exit(2); }
const SERVER = '$HOME/sk-target/debug/shikisha-server';
const staged = path.join(process.env.TEMP, 'sk-boards-button-config.json');
fs.writeFileSync(staged, JSON.stringify({
  language: 'en',
  remote: { enabled: true, bind: '0.0.0.0', port: PORT, allow_public: true },
  desks: [{ name: 'Srv', id: 'srv', folders: [{ cwd: '/tmp', tabs: [{ name: 'sh', id: 'sh', command: 'bash' }] }] }],
}));
const stagedWsl = '/mnt/' + staged[0].toLowerCase() + staged.slice(2).replace(/\\/g, '/');
wsl(`pkill -f '[s]k-target/debug/shikisha-server.*' ; rm -rf ${SRV_HOME} ; mkdir -p ${SRV_HOME}/config && cp ${stagedWsl} ${SRV_HOME}/config/config.json`);
const server = spawn('wsl', ['-e', 'bash', '-c', `SHIKISHA_HOME=${SRV_HOME} ${SERVER} > ${SRV_HOME}/out.log 2>&1`], { stdio: 'ignore', env: { ...process.env, MSYS_NO_PATHCONV: '1' } });

let cdp;
const start = async (boards, tokens) => {
  stopOurs();
  await sleep(1000);
  fs.rmSync(RUN, { recursive: true, force: true });
  for (const d of [APP, LOCAL]) fs.mkdirSync(d, { recursive: true });
  const st = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
  if (st.status !== 0) throw new Error('staging failed: ' + st.stdout + st.stderr);
  fs.mkdirSync(path.join(APP, 'config'), { recursive: true });
  fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
    language: 'en', remote: { enabled: false }, resident: false, boards,
    desks: [{ name: 'Here', id: 'here', folders: [{ cwd: RUN, tabs: [{ name: 'sh', id: 'sh', command: ['cmd.exe'] }] }] }],
  }, null, 2));
  fs.writeFileSync(path.join(APP, 'config', 'secrets.json'), JSON.stringify({ tokens }, null, 2));
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
  env.LOCALAPPDATA = LOCAL;
  env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
  spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
  // The window's own page, by its DevTools port
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', 'shell', 'EBWebView', 'DevToolsActivePort');
  let target;
  await until(async () => {
    if (!fs.existsSync(f)) return false;
    const port = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
    target = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === 'page');
    return !!target;
  }, 'the window\'s page', 60000);
  cdp = (await connectCdp(target, { timeout: 8000 })).run;
  await until(() => cdp('!!(S && S.tabs && S.tabs.length)'), 'the board to draw', 30000);
};

try {
  await until(() => fetch(BOARD + '/pair/status', { signal: AbortSignal.timeout(3000) }).then((r) => r.status === 403), 'the server version to come up', 60000);
  await until(() => wsl(`test -s ${SRV_HOME}/data/board-address && echo yes`).stdout.includes('yes'), 'its address', 30000);
  const code = (wsl(`SHIKISHA_HOME=${SRV_HOME} ${SERVER} pair`).stdout.match(/[A-Z0-9]{4}-[A-Z0-9]{4}/) || [])[0];
  const paired = await (await fetch(BOARD + '/pair', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ code, name: 'test PC' }) })).json();
  if (!paired.ok || !paired.key) throw new Error('could not pair: ' + JSON.stringify(paired));

  console.log('1. paired with one: the 🖥️ opens it');
  await start([{ name: 'wsl', url: BOARD }], { 'board/wsl/key': paired.key });
  check(await cdp('!!document.querySelector(".boardsbtn")'), 'the 🖥️ is at the foot of the sidebar');
  check(/wsl/.test(await cdp('document.querySelector(".boardsbtn").title')), 'it says which it opens: ' + await cdp('document.querySelector(".boardsbtn").title'));
  await cdp('document.querySelector(".boardsbtn").click(), true');
  await until(() => log().includes(`pairing: a window opened on ${BOARD}`), 'the board\'s window', 30000).catch(() => {});
  check(log().includes(`pairing: a window opened on ${BOARD}`), 'pressed, the board opens in a window of its own');
  if (failures) console.log('       log: ' + log().split(/\r?\n/).filter((l) => /pairing|board|secret/i.test(l)).slice(-8).join(' | '));
  const opened = ps('-Command', `@(Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'SHIKISHA-TERM.exe' -and $_.CommandLine -like '*--connect*127.0.0.1:${PORT}*' }).Count`).stdout.trim();
  check(Number(opened) >= 1, `the window is there (${opened} running with --connect ${BOARD})`);

  console.log('2. two, one without its key: a list, and the reason on the board');
  await start([{ name: 'wsl', url: BOARD }, { name: 'nokey', url: 'http://127.0.0.1:8797' }], { 'board/wsl/key': paired.key });
  await cdp('document.querySelector(".boardsbtn").click(), true');
  await sleep(300);
  const rows = await cdp('[...document.querySelectorAll(".fmenu > div")].map(d => d.textContent).join(" | ")');
  check(/wsl/.test(rows) && /nokey/.test(rows) && /Manage/.test(rows), 'pressed, it is a list of both and the settings: ' + rows);
  await cdp('[...document.querySelectorAll(".fmenu > div")].find(d => d.textContent.includes("nokey")).click(), true');
  const noKey = JSON.parse(fs.readFileSync(path.join(ROOT, 'lang', 'en.json'), 'utf8'))['err.board.no_key'];
  let flash = '';
  await until(async () => (flash = String(await cdp('S && S.flash || ""'))).includes(noKey), 'the reason', 15000).catch(() => {});
  check(flash.includes(noKey) && !/a window opened on http:\/\/127\.0\.0\.1:8797/.test(log()), 'the one without a key says why, and opens nothing: ' + flash);
} catch (e) {
  failures += 1;
  console.error('stopped: ' + (e.stack || e));
} finally {
  stopOurs();
  server.kill();
  wsl(`pkill -f '[s]k-target/debug/shikisha-server.*'; rm -rf ${SRV_HOME}`);
  await sleep(1000);
  fs.rmSync(RUN, { recursive: true, force: true });
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
