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
 *     node tools/debug/settings-shoot.mjs <scenes.mjs> --remote-http
 *
 * `--remote-http` opens through a test hostname mapped to loopback in this
 * Chrome only. The page has the same browser API limits as a phone on HTTP;
 * localhost alone is trusted by browsers and would miss those failures.
 *
 * A scene file default-exports { config, scenes, langs, looks, sizes, init }:
 * `config` is the settings to start from, and each scene is the JavaScript
 * that puts the page into the state being judged (it may return a promise).
 * A scene may instead be `{ query, run }`: the page is opened with `query`
 * added to its address, the way the board opens it on one place, and `run`
 * (optional) is the JavaScript.
 * `init` runs before the page's scripts, for fixtures that must not reach the
 * machine's real sign-ins. Other requests still go to the isolated server.
 *
 * Needs Chrome and cargo. The light scheme's colours are laid over the page
 * the way page_dump lays them, so the machine's own scheme does not decide it.
 */
import {findCargo, startChrome} from './chrome.mjs';
import fs from 'node:fs';
import path from 'node:path';
import http from 'node:http';
import { spawn, spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const OUT = path.join(ROOT, 'target', 'shots');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(1); };

/** The settings page for one language, served until `stop` */
async function serve(lang, config) {
  fs.mkdirSync(OUT, { recursive: true });
  const file = path.join(OUT, `settings-start-${process.pid}-${lang}.json`);
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
  fs.unlinkSync(file);
  if (!remoteHttp) return { url, stop: () => proc.kill() };
  // The settings server deliberately rejects non-loopback Host headers.
  // Forward only to this isolated server, as the remote board's proxy does;
  // do not weaken that protection to make a test hostname work.
  const local = new URL(url);
  const proxy = http.createServer((req, res) => {
    const upstream = http.request({ hostname: '127.0.0.1', port: local.port,
      path: req.url, method: req.method,
      headers: {...req.headers, host: local.host, 'X-Remote-Client': '1'} }, reply => {
      res.writeHead(reply.statusCode, reply.headers); reply.pipe(res);
    });
    upstream.on('error', () => { res.writeHead(502); res.end('Settings server unavailable'); });
    req.pipe(upstream);
  });
  await new Promise((resolve, reject) => {
    proxy.once('error', reject); proxy.listen(0, '127.0.0.1', resolve);
  });
  const shown = new URL(url);
  shown.port = String(proxy.address().port);
  return { url: shown.href, stop: () => { proxy.closeAllConnections(); proxy.close(); proc.kill(); } };
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

const connect = () => startChrome({args:remoteHttp ? ['--host-resolver-rules=MAP settings.test 127.0.0.1', '--no-proxy-server'] : []});

const file = process.argv[2] || die('say which scenes: node tools/debug/settings-shoot.mjs <scenes.mjs>');
const only = process.argv.includes('--only') ? process.argv[process.argv.indexOf('--only') + 1] : null;
const remoteHttp = process.argv.includes('--remote-http');
const spec = (await import(pathToFileURL(path.resolve(file)).href)).default;
const langs = spec.langs || ['en', 'ja'];
const looks = spec.looks || ['dark', 'light'];
const sizes = spec.sizes || [['wide', 1280, 860], ['phone', 390, 820]];

const light = looks.includes('light') ? lightColours() : '';
const chrome = await connect();
await chrome.send('Page.enable');
if (spec.init) await chrome.send('Page.addScriptToEvaluateOnNewDocument', { source: spec.init });
let taken = 0;
try {
for (const lang of langs) {
  const server = await serve(lang, spec.config || {});
  try {
    for (const [name, scene] of Object.entries(spec.scenes)) {
      if (only && name !== only) continue;
      for (const look of looks) {
        for (const [size, w, h] of sizes) {
          await chrome.send('Emulation.setDeviceMetricsOverride',
            { width: w, height: h, deviceScaleFactor: 2, mobile: size === 'phone' });
          const query = typeof scene === 'object' ? scene.query || '' : '';
          const run = typeof scene === 'object' ? scene.run || '"ok"' : scene;
          const pageUrl = new URL(server.url + (query ? (server.url.includes('?') ? '&' : '?') + query : ''));
          if (remoteHttp) pageUrl.hostname = 'settings.test';
          await chrome.send('Page.navigate', { url: pageUrl.href });
          // Loaded once the settings have been read and drawn
          const ready = () => chrome.run('typeof desks !== "undefined" && document.querySelector("#detail")?.childElementCount > 0').catch(() => false);
          for (let i = 0; i < 80; i++) {
            if (await ready()) break;
            await sleep(150);
          }
          if (!await ready()) {
            throw new Error('Settings did not load: ' + await chrome.run('document.body.innerText.slice(0, 200)'));
          }
          if (remoteHttp && !await chrome.run('REMOTE && !isSecureContext && typeof crypto.randomUUID === "undefined"')) {
            throw new Error('Remote HTTP test did not reach an insecure browser context');
          }
          if (look === 'light') {
            await chrome.run('(() => { const s = document.createElement("style"); s.textContent = '
              + JSON.stringify(light) + '; document.head.append(s); })()');
          }
          await chrome.run(run);
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
} finally { chrome.stop(); }
if (!taken) die(only ? 'no scene called ' + only : 'the scene file has no scenes');
