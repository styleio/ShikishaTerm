/**
 * Is the page a phone is watching drawn at the size that phone asked for?
 *
 *     node tools/debug/relay-shape.win.mjs --at <a running copy's folder>
 *
 * Needs Windows, Chrome, and a copy of the app already running out of that
 * folder with the relay switched on (`tools/debug/instance.win.ps1` makes
 * one). Nothing here ships and nothing is installed.
 *
 * Why it exists. What a phone is shown is drawn on the PC at a size worked
 * out from the shape the phone reports, and that arithmetic has inputs that
 * change under it -- the window being resized, the window being put away.
 * When it goes wrong the PC looks fine and the phone gets a page 128 pixels
 * wide blown up to fill the screen, so the only way to see it is to be the
 * phone. This stands in for one: headless Chrome at 412x915 opens the relay
 * with the token, says how big a picture it is actually being sent, and then
 * does the thing that broke it -- puts the app's window away, the ordinary
 * move of somebody who has walked off with their phone -- and asks again.
 *
 * What it prints is the picture's size at each step. They should be the same,
 * and neither should be small.
 */
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const argAt = process.argv.indexOf('--at');
if (argAt < 0) {
  console.error('say which running copy: --at <folder>');
  process.exit(2);
}
const ROOT = path.resolve(process.argv[argAt + 1]);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Where that copy answers, and what it wants to hear
const settings = JSON.parse(fs.readFileSync(path.join(ROOT, 'config', 'config.json'), 'utf8'));
const port = (settings.remote && settings.remote.port) || 8787;
const token = fs.readFileSync(path.join(ROOT, 'data', 'remote-token'), 'utf8').trim();
const exe = path.join(ROOT, 'SHIKISHA-TERM.exe');

const pid = Number(
  execFileSync('powershell', ['-NoProfile', '-Command',
    `(Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue |` +
    ` Where-Object { $_.Path -eq '${exe}' } | Select-Object -First 1).Id`,
  ], { encoding: 'utf8' }).trim());
if (!pid) {
  console.error('nothing is running out of ' + ROOT);
  process.exit(2);
}

// The window, shown or put away. ShowWindow rather than the app's own bar
// button, so that this says nothing about which of the two paths is taken
const show = (how, tag) => execFileSync('powershell', ['-NoProfile', '-Command',
  `$s = '[DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);';` +
  ` $u = Add-Type -MemberDefinition $s -Name ${tag} -Namespace N${tag} -PassThru;` +
  ` $u::ShowWindow((Get-Process -Id ${pid}).MainWindowHandle, ${how}) | Out-Null`,
], { encoding: 'utf8' });

function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  for (const base of [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA]) {
    if (!base) continue;
    const p = path.join(base, 'Google', 'Chrome', 'Application', 'chrome.exe');
    if (fs.existsSync(p)) return p;
  }
  throw new Error('no Chrome found; set CHROME');
}

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'standin-phone-'));
const chrome = spawn(findChrome(), [
  '--headless=new',
  '--remote-debugging-port=9333',
  '--user-data-dir=' + profile,
  '--window-size=412,915',
  '--autoplay-policy=no-user-gesture-required',
  '--no-first-run',
  'about:blank',
], { stdio: 'ignore' });

const { attach } = await import(
  new URL('../../.private/doc/proto/webrtc-http/cdp.mjs', import.meta.url).href);

// One visit, from a phone-shaped screen, reporting what it is being sent
async function watch(secs) {
  const cdp = await attach(9333);
  await cdp.call('Emulation.setDeviceMetricsOverride',
    { width: 412, height: 915, deviceScaleFactor: 2.625, mobile: true });
  await cdp.call('Page.enable');
  await cdp.call('Runtime.enable');
  await cdp.call('Page.navigate', { url: `http://127.0.0.1:${port}/?t=${token}` });
  await sleep(secs * 1000);
  const m = await cdp.call('Runtime.evaluate', {
    returnByValue: true,
    expression: `(() => {
      const v = document.getElementById('castv');
      const c = document.getElementById('cast');
      return {
        // What the PC is drawing. As JPEG it is the canvas; as video it is
        // the picture inside the video element
        drawn: v && !v.hidden && v.videoWidth
          ? v.videoWidth + 'x' + v.videoHeight
          : (c ? c.width + 'x' + c.height : 'nothing'),
        as: v && !v.hidden ? 'video' : 'JPEG',
        // What this screen asked for
        asked: c ? Math.round(c.clientWidth) + 'x' + Math.round(c.clientHeight) : '?',
      };
    })()`,
  });
  cdp.close();
  return m.result.result.value;
}

let bad = false;
const width = (s) => Number(String(s.drawn).split('x')[0]) || 0;
try {
  show(9, 'Up');                       // SW_RESTORE, so the first look is of a window on screen
  await sleep(2500);
  const open = await watch(14);
  console.log('window on screen: drawn', open.drawn, 'as', open.as, ', this screen is', open.asked);

  show(6, 'Away');                     // SW_MINIMIZE
  await sleep(3000);
  const away = await watch(16);
  console.log('window put away:  drawn', away.drawn, 'as', away.as, ', this screen is', away.asked);

  if (width(open) < 320) { bad = true; console.log('FAIL: the page is tiny with the window on screen'); }
  if (width(away) < 320) { bad = true; console.log('FAIL: putting the window away shrank the page'); }
  if (!bad) console.log('PASS: the page keeps its size whether the window is watched or not');
} finally {
  show(9, 'Back');
  chrome.kill();
  await sleep(500);
  fs.rmSync(profile, { recursive: true, force: true });
}
process.exit(bad ? 1 : 0);
