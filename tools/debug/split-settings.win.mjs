/**
 * The settings, the ? and the master password from the window of a program
 * split in two (Settings > Basic > Screen and work).
 *
 * Split, the window is a page on the board like a phone's, opened with the key
 * the runtime handed it. It reaches the settings and the ? by the same doors a
 * phone does (the proxied settings page and the guide page), and is checked to
 * reach them for real -- not that a button exists, but that the page behind it
 * loads, answers, and saves. The master password is the one thing it may not
 * be handed: a split runtime has no window of its own to ask in, and the answer
 * does not travel over the board. What is checked there is that the person is
 * told so, and where it can be typed.
 *
 *     cargo build
 *     node tools/debug/split-settings.win.mjs
 *
 * Checked:
 *   - the gear on one tab's settings opens them over the board, in a frame of the
 *     board's own origin, and the page in it has loaded the settings
 *   - a change typed there and saved is in config/config.json
 *   - the whole settings (the gear with nothing named) take the window and come
 *     back to the board with the browser's back
 *   - the ? opens the guide beside the board, and it has loaded
 *   - a choose-a-folder field offers the in-page chooser, not a dialog on no screen
 *   - with the secrets encrypted, the window says the master password is not
 *     taken here and where it is; setting one from the menu says the same
 *
 * `SPLIT_LANG=ja` reads the screen in Japanese. Needs Windows and Node.
 * Isolated: its own folder and LOCALAPPDATA; the app is stopped on the way out.
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const RUN = path.join(os.tmpdir(), 'sk-split-settings');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const LANG = process.env.SPLIT_LANG || 'en';
const readLang = (l) => JSON.parse(fs.readFileSync(path.join(ROOT, 'lang', l + '.json'), 'utf8'));
const L = { ...readLang('en'), ...(LANG === 'en' ? {} : readLang(LANG)) };

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and ($_.CommandLine + '') -like '*sk-split-settings*' } | ` +
  `ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);
const until = async (test, what, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(150); }
  throw new Error('timed out waiting for ' + what);
};
const freePort = () => new Promise((res) => {
  const srv = net.createServer().listen(0, '127.0.0.1', () => { const p = srv.address().port; srv.close(() => res(p)); });
});

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

const PHONE_PORT = await freePort();
const PHONE_KEY = 'splitsettings0123456789';

/** A copy of its own, started with `secrets` as its secrets file */
async function start(secrets) {
  stopApp();
  await sleep(800);
  fs.rmSync(RUN, { recursive: true, force: true });
  for (const d of [APP, WORK, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
  const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
  if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
  fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
  fs.writeFileSync(CONFIG, JSON.stringify({
    language: LANG,
    split: true,
    remote: { enabled: true, bind: '127.0.0.1', port: PHONE_PORT, sticky_token: true, fixed_token: PHONE_KEY },
    ...(secrets ? { secrets: 'secrets.json' } : {}),
    desks: [{ name: 'Check', id: 'check', folders: [{ cwd: WORK, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }] }],
  }, null, 2));
  if (secrets) fs.writeFileSync(path.join(APP, 'config', 'secrets.json'), secrets);
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
  env.LOCALAPPDATA = LOCAL;
  env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
  spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
  return connect(await windowPage());
}

const portOf = (envName) => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', envName, 'EBWebView', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return null;
  const n = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};
const targetsOf = async (port) => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json());

/** The window's page: the board, opened with the key the window was handed */
async function windowPage(what = /[?&]here=/) {
  let found;
  await until(async () => {
    const base = path.join(LOCAL, 'ShikishaTerm', 'webview2');
    if (!fs.existsSync(base)) return false;
    for (const envName of fs.readdirSync(base)) {
      const p = portOf(envName);
      if (!p) continue;
      const pages = (await targetsOf(p).catch(() => [])).filter((t) => t.type === 'page');
      found = pages.find((t) => what.test(t.url));
      if (found) return true;
    }
    return false;
  }, 'the window\'s page', 60000);
  return found;
}

async function connect(target) {
  const {ws, send, run} = await connectCdp(target);
  const shot = async (label) => {
    const r = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, `split-settings-${LANG}-${label}.png`), Buffer.from(r.data, 'base64'));
  };
  return { ws, send, run, shot };
}

// The frame's own document, when it is of the board's origin (it must be: the
// settings and the guide are served by the board's own door)
const inFrame = (id, expr) => `(() => { const f = document.getElementById(${JSON.stringify(id)});
  if (!f || !f.contentWindow || !f.contentDocument || !f.contentDocument.body) return null;
  return f.contentWindow.eval(${JSON.stringify(expr)}); })()`;

let board;
try {
  console.log('starting this checkout\'s build, split in two, isolated');
  board = await start(null);
  await until(() => board.run('!!S && Array.isArray(S.tabs) && S.tabs.length > 0'), 'the board', 30000);
  check(await board.run('REMOTE === true && AT_PC === true'), 'the window is a page on the board, and knows it is at this PC');

  console.log('1. one tab\'s settings, over the board');
  await board.run('openSettings(null, false, null, S.tabs.find(t => t.name === "shell")); true');
  await until(() => board.run('!!document.getElementById("cfglayer")'), 'the settings frame');
  const src = await board.run('document.getElementById("cfglayer").getAttribute("src")');
  check(/^cfg\?/.test(src), 'the frame is the board\'s own settings door: ' + src.replace(/t=[^&]+/, 't=…'));
  await until(() => board.run(inFrame('cfglayer', '!!document.querySelector("input, select, button")')), 'the settings page to load', 20000);
  check(await board.run(inFrame('cfglayer', 'location.pathname.startsWith("/cfg")')), 'the settings page has loaded in it');
  check(!(await board.run(inFrame('cfglayer', '/[?&]t=/.test(location.search)'))), 'the token is not left in the frame\'s address');
  await board.shot('1-tab-settings');
  await board.run('closeCfgLayer(); true');

  console.log('2. a change saved from there is in the settings file');
  await board.run('openSettings("basic", false, null, null); true');
  await until(() => board.run(inFrame('cfglayer', 'typeof save === "function" && typeof current === "object" && Object.keys(current).length > 0')), 'the settings to be ready', 20000);
  // Found by the key it writes: the size of the terminal's text
  const saved = await board.run(inFrame('cfglayer', '(async () => { current.font_size = 17; await save(); return "saved"; })()'));
  await until(() => JSON.parse(fs.readFileSync(CONFIG, 'utf8')).font_size === 17, 'font_size in config.json', 10000).catch(() => {});
  check(JSON.parse(fs.readFileSync(CONFIG, 'utf8')).font_size === 17, 'the saved value is in config/config.json (' + saved + ')');
  await board.run('closeCfgLayer(); true');

  console.log('3. the whole settings, and back');
  await board.run('openSettings(); true');
  const whole = await windowPage(/\/cfg/);
  check(!!whole, 'the whole settings took the window');
  const wholePage = await connect(whole);
  await until(() => wholePage.run('!!document.querySelector("input, select, button")'), 'the whole settings to load', 20000);
  check(!(await wholePage.run('/[?&]t=/.test(location.search)')), 'the token is not left in the address bar');
  await wholePage.shot('3-whole-settings');
  await wholePage.run('history.back(); true');
  board = await connect(await windowPage());
  await until(() => board.run('!!S && Array.isArray(S.tabs)'), 'the board again', 20000);
  check(true, 'the browser\'s back came to the board');

  console.log('4. the ?');
  await board.run('toggleGuide(); true');
  await until(() => board.run('!!document.getElementById("guideframe")'), 'the guide frame');
  await until(() => board.run(inFrame('guideframe', '!!document.body && document.body.innerText.length > 20')), 'the guide to load', 20000);
  check(await board.run(inFrame('guideframe', 'location.pathname.startsWith("/guide")')), 'the guide has loaded beside the board');
  await board.shot('4-guide');
  await board.run('closeGuide(); true');

  console.log('5. choosing a folder is the page\'s own chooser');
  // The window of a split program has no runtime window to hang a system
  // dialog on; the chooser has to be the one that browses in the page
  await board.run('openSettings("basic", false, null, null); true');
  await until(() => board.run(inFrame('cfglayer', 'typeof choosePath === "function"')), 'the settings page', 20000);
  const how = await board.run(inFrame('cfglayer', 'typeof REMOTE !== "undefined" ? (REMOTE ? "in-page" : "system") : "unknown"'));
  check(how === 'in-page', 'a folder is chosen in the page (' + how + ')');
  await board.run('closeCfgLayer(); true');

  console.log('6. the master password, with the secrets encrypted');
  // An encrypted file that is not ours to open: the check is what is said
  const locked = JSON.stringify({ magic: 'shikisha-enc-v1', salt: 'AAAAAAAAAAAAAAAAAAAAAA==', nonce: 'AAAAAAAAAAAAAAAA', data: 'AAAA' });
  board.ws.close();
  board = await start(locked);
  await until(() => board.run('!!S && Array.isArray(S.tabs)'), 'the board', 30000);
  const said = L['prompt.password.split'];
  check(!!said, 'there are words for it: prompt.password.split');
  const seen = await until(() => board.run(`JSON.stringify(S.flash || "") + " " + document.body.innerText`).then((t) => t.includes(said.slice(0, 20))), 'the words', 20000).then(() => true).catch(() => false);
  check(seen, 'the window says the master password is not taken here, and where it is');
  check(!(await board.run('!document.getElementById("veil").hidden && !!document.querySelector("#veil input[type=password]")')), 'no password box is put up on the board');
  await board.shot('6-password');
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  stopApp();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
