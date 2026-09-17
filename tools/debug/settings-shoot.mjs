/**
 * Photograph the settings page, in both languages and schemes and at window
 * and phone width, over a settings file of the scene's own.
 *
 * The settings page is served by the app over the settings it runs on, and
 * those are somebody's real ones. src/bin/settings_serve.rs serves the same
 * page from the same code over a copy in a temporary folder, so a scene can
 * type into it and press its buttons -- including the ones that ask the app
 * something, which a page opened from a file could not. Pictures land in
 * target/shots, named settings-<scene>-<lang>-<look>-<size>.png.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-servers.mjs
 *     node tools/debug/settings-shoot.mjs <scenes.mjs> --only <scene>
 *
 * A scene file default-exports { config, scenes, langs, looks, sizes }:
 * `config` is the settings to start from, and each scene is the JavaScript
 * that puts the page into the state being judged (it may return a promise).
 *
 * Needs Chrome and cargo. The light scheme's colours are laid over the page
 * the way page_dump lays them, so the machine's own scheme does not decide it.
 */
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { spawn, spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const OUT = path.join(ROOT, 'target', 'shots');
const PORT = 9334;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(1); };

function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  const guesses = process.platform === 'win32'
    ? [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA]
      .filter(Boolean).map((b) => path.join(b, 'Google/Chrome/Application/chrome.exe'))
    : ['/usr/bin/google-chrome', '/usr/bin/chromium', '/usr/bin/chromium-browser'];
  return guesses.find((p) => fs.existsSync(p)) || die('Chrome was not found; say where with CHROME=<path>');
}
function findCargo() {
  const beside = path.join(os.homedir(), '.cargo', 'bin', process.platform === 'win32' ? 'cargo.exe' : 'cargo');
  return fs.existsSync(beside) ? beside : 'cargo';
}

/** The settings page for one language, served until `stop` */
async function serve(lang, config) {
  fs.mkdirSync(OUT, { recursive: true });
  const file = path.join(OUT, 'settings-start.json');
  fs.writeFileSync(file, JSON.stringify(config, null, 2));
  // Built first and run as itself: stopping `cargo run` leaves the program it
  // started still serving on Windows
  const built = spawnSync(findCargo(), ['build', '--quiet', '--bin', 'settings_serve'], { cwd: ROOT, stdio: 'inherit' });
  if (built.status !== 0) die('settings_serve did not build');
  const exe = path.join(ROOT, 'target', 'debug', 'settings_serve' + (process.platform === 'win32' ? '.exe' : ''));
  const proc = spawn(exe, [lang, file], { cwd: ROOT, stdio: ['ignore', 'pipe', 'inherit'] });
  let said = '';
  proc.stdout.on('data', (d) => { said += d; });
  for (let i = 0; i < 200 && !said.includes('\n'); i++) await sleep(100);
  const url = said.split('\n')[0].trim();
  if (!url.startsWith('http')) die('the settings page did not start');
  return { url, stop: () => proc.kill() };
}

/** The light scheme's colours, as the board page's dump carries them */
function lightColours() {
  const made = spawnSync(findCargo(), ['run', '--quiet', '--bin', 'page_dump', '--', 'en', 'light'],
    { cwd: ROOT, maxBuffer: 1 << 28 });
  if (made.status !== 0) die('page_dump failed: ' + made.stderr);
  const html = String(made.stdout);
  const at = html.lastIndexOf('<style>:root{');
  return html.slice(at + '<style>'.length, html.indexOf('</style>', at));
}

async function connect() {
  const proc = spawn(findChrome(), [
    '--headless=new', '--remote-debugging-port=' + PORT,
    '--user-data-dir=' + path.join(OUT, 'chrome-settings-profile'),
    '--no-first-run', '--no-default-browser-check', '--hide-scrollbars', 'about:blank',
  ], { stdio: 'ignore' });
  let list;
  for (let i = 0; i < 80 && !list; i++) {
    try { list = await (await fetch('http://127.0.0.1:' + PORT + '/json/list')).json(); } catch { await sleep(250); }
  }
  if (!list) die('Chrome never answered on its debugging port');
  const ws = new WebSocket(list.find((t) => t.type === 'page').webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r, { once: true }));
  let id = 0;
  const waiting = new Map();
  ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data);
    if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
  });
  const send = (method, params = {}) => new Promise((res, rej) => {
    const n = ++id;
    waiting.set(n, (m) => (m.error ? rej(new Error(method + ': ' + JSON.stringify(m.error))) : res(m.result)));
    ws.send(JSON.stringify({ id: n, method, params }));
  });
  const run = async (expression) => {
    const r = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  };
  return { send, run, stop: () => { ws.close(); proc.kill(); } };
}

const file = process.argv[2] || die('say which scenes: node tools/debug/settings-shoot.mjs <scenes.mjs>');
const only = process.argv.includes('--only') ? process.argv[process.argv.indexOf('--only') + 1] : null;
const spec = (await import(pathToFileURL(path.resolve(file)).href)).default;
const langs = spec.langs || ['en', 'ja'];
const looks = spec.looks || ['dark', 'light'];
const sizes = spec.sizes || [['wide', 1280, 860], ['phone', 390, 820]];

const light = looks.includes('light') ? lightColours() : '';
const chrome = await connect();
await chrome.send('Page.enable');
let taken = 0;
for (const lang of langs) {
  const server = await serve(lang, spec.config || {});
  try {
    for (const [name, scene] of Object.entries(spec.scenes)) {
      if (only && name !== only) continue;
      for (const look of looks) {
        for (const [size, w, h] of sizes) {
          await chrome.send('Emulation.setDeviceMetricsOverride',
            { width: w, height: h, deviceScaleFactor: 2, mobile: size === 'phone' });
          await chrome.send('Page.navigate', { url: server.url });
          // Loaded once the settings have been read and drawn
          for (let i = 0; i < 80; i++) {
            const ready = await chrome.run('typeof desks !== "undefined" && document.querySelector("#detail") && document.querySelector("#detail").childElementCount > 0').catch(() => false);
            if (ready) break;
            await sleep(150);
          }
          if (look === 'light') {
            await chrome.run('(() => { const s = document.createElement("style"); s.textContent = '
              + JSON.stringify(light) + '; document.head.append(s); })()');
          }
          await chrome.run(scene);
          await sleep(900);
          const shot = await chrome.send('Page.captureScreenshot', { format: 'png' });
          const out = path.join(OUT, 'settings-' + name + '-' + lang + '-' + look + '-' + size + '.png');
          fs.writeFileSync(out, Buffer.from(shot.data, 'base64'));
          console.log(path.relative(ROOT, out));
          taken += 1;
        }
      }
    }
  } finally {
    server.stop();
  }
}
chrome.stop();
if (!taken) die(only ? 'no scene called ' + only : 'the scene file has no scenes');
