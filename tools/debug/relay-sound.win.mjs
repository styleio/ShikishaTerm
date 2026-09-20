/**
 * Does the sound of the watched page reach the far end, and nothing else's?
 *
 *     cargo build --release --bin SHIKISHA-TERM
 *     node tools/debug/relay-sound.win.mjs [--seconds 20]
 *
 * Needs Windows, Chrome and Node. Its own copy of the app, its own folder,
 * its own port; nothing of a copy somebody is using is read or stopped.
 *
 * What it does. The copy is pointed at a page that plays a steady tone. A
 * Chrome stands in for the phone, opens the relay, waits for the video to
 * carry the picture, then taps the speaker the way a finger would -- and
 * reads the browser's own figures for the sound it is receiving: how many
 * packets, how many seconds of it, and how loud.
 *
 * Then the same again with the tone stopped, because "we hear something" is
 * only half of it. The other half is that a page playing nothing sends
 * nothing, and that nothing else on this machine is heard: the tone is played
 * by ANOTHER browser for the second half, and what the phone hears then must
 * be silence.
 */
import fs from 'node:fs';
import os from 'node:os';
import http from 'node:http';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-sound');
const APP = path.join(RUN, 'app');
const CONFIG = path.join(APP, 'config', 'config.json');
const RELAY_PORT = 8797;
const PAGE_PORT = 8904;
const TOKEN = 'sound-token-0123456789abcdef';
const SECONDS = Number((process.argv[process.argv.indexOf('--seconds') + 1]) || 20);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
const ps = (...a) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...a], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);

// ── The page that makes a noise ───────────────
// A tone rather than music: one frequency at one level is a thing a figure
// can be checked against, and silence is unmistakable next to it
const TONE = `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>tone</title>
<style>html,body{margin:0;height:100%;background:#101014;color:#ddd;
font:20px system-ui,sans-serif;display:flex;align-items:center;justify-content:center}</style>
</head><body><div id="s">starting</div><script>
  const ctx = new AudioContext({ sampleRate: 48000 });
  const osc = ctx.createOscillator();
  const gain = ctx.createGain();
  osc.frequency.value = 440;
  gain.gain.value = 0.3;
  osc.connect(gain).connect(ctx.destination);
  osc.start();
  ctx.resume().catch(() => {});
  const s = document.getElementById('s');
  const show = () => { s.textContent = '440Hz at ' + gain.gain.value + ', ' + ctx.state; };
  show();
  // The page is told to stop by asking for it, so that the same page can be
  // both halves of the check
  setInterval(async () => {
    try {
      const want = await (await fetch('/level')).text();
      if (Number(want) !== gain.gain.value) { gain.gain.value = Number(want); show(); }
    } catch (e) {}
  }, 300);
</script></body></html>`;

let level = 0.3;
const server = http.createServer((req, res) => {
  if (req.url.startsWith('/level')) { res.writeHead(200, { 'content-type': 'text/plain' }); res.end(String(level)); return; }
  res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
  res.end(TONE);
}).listen(PAGE_PORT, '127.0.0.1');

// ── A copy of the app ─────────────────────────
const exe = path.join(ROOT, 'target', 'release', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no release build -- cargo build --release --bin SHIKISHA-TERM');

console.log('starting a copy of this build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, path.join(RUN, 'localappdata')]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: 'ja',
  remote: { enabled: true, bind: '127.0.0.1', port: RELAY_PORT, sticky_token: true, fixed_token: TOKEN },
  desks: [{ name: 'Sound', id: 'sound', browsers: [{ id: 'tone', url: `http://127.0.0.1:${PAGE_PORT}/` }] }],
}, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), [], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
for (let i = 0; i < 160; i++) {
  try { if ((await fetch(`http://127.0.0.1:${RELAY_PORT}/?t=${TOKEN}`)).ok) break; } catch { /* not yet */ }
  await sleep(250);
}
console.log('the copy is up');

// ── The tab with the tone in it ───────────────
// Asked for over the relay's own door rather than from inside the viewer:
// choosing a tab from the page means reloading it afterwards, and a reload
// ends the very connection this is about to measure
{
  const door = `http://127.0.0.1:${RELAY_PORT}`;
  const first = await fetch(`${door}/?t=${TOKEN}`);
  const cookie = (first.headers.getSetCookie?.() || []).join('; ');
  await first.text();
  const state = await (await fetch(`${door}/api/state?t=${TOKEN}`, { headers: { cookie } })).json();
  // The rail's own list: terminals and browsers together, which is what a
  // finger picks from
  const tabs = (state.ui && state.ui.tabs) || state.tabs || [];
  const tab = tabs.find((t) => t.name === 'tone' || t.id === 'tone');
  if (!tab) die('the copy has no tab called tone: ' + JSON.stringify(state).slice(0, 200));
  await fetch(`${door}/api/intent?t=${TOKEN}`, {
    method: 'POST',
    headers: { cookie, 'content-type': 'application/json' },
    body: JSON.stringify({ kind: 'select', tab: tab.index }),
  });
  console.log('the tone tab is the one on screen');
  await sleep(1500);
}

// ── The viewer ────────────────────────────────
const { attach } = await import(
  new URL('../../.private/doc/proto/webrtc-http/cdp.mjs', import.meta.url).href);

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'sound-'));
const chrome = spawn(process.env.CHROME
  || path.join(process.env.PROGRAMFILES, 'Google', 'Chrome', 'Application', 'chrome.exe'), [
  '--remote-debugging-port=9355', '--user-data-dir=' + profile,
  '--window-size=412,915', '--autoplay-policy=no-user-gesture-required',
  '--no-first-run', '--no-default-browser-check', 'about:blank',
], { stdio: 'ignore' });
await sleep(2500);

const cdp = await attach(9355);
await cdp.call('Page.enable');
await cdp.call('Runtime.enable');
await cdp.call('Emulation.setDeviceMetricsOverride',
  { width: 412, height: 915, deviceScaleFactor: 2, mobile: true });
await cdp.call('Page.navigate', { url: `http://127.0.0.1:${RELAY_PORT}/?t=${TOKEN}` });
// Long enough for the picture to start and for video to take over from the
// line it opened first
await sleep(14000);

const says = async (expression) => {
  const m = await cdp.call('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
  const r = m.result && m.result.result;
  if (!r) return null;
  if (m.result.exceptionDetails) {
    return 'the page threw: ' + JSON.stringify(m.result.exceptionDetails).slice(0, 200);
  }
  // A value that came back as an object rather than the string it was asked
  // for is a promise the protocol did not unwrap; say so instead of parsing it
  return typeof r.value === 'string' || typeof r.value === 'number' || r.value == null
    ? r.value
    : JSON.stringify(r.value);
};

// Tap the speaker, the way a finger does
const tapped = await says(`(() => {
  const b = document.getElementById('castear');
  if (!b || b.hidden) return 'no speaker to tap: ' + (b ? 'hidden' : 'missing');
  b.click();
  return 'tapped';
})()`);
console.log('the speaker:', tapped);
// The line for sound exists from the start, but nothing travels down it until
// the PC has been told and has started listening. Give it a moment, or the
// first reading is of a line that has carried nothing yet
await sleep(3000);

// What the browser says it is receiving, after a while of listening
const SOUND_STATS = `(async () => {
  if (typeof videoPc === 'undefined' || !videoPc) return 'no connection';
  let r = null;
  (await videoPc.getStats()).forEach((x) => { if (x.type === 'inbound-rtp' && x.kind === 'audio') r = x; });
  if (!r) return 'no sound line';
  return JSON.stringify({ packets: r.packetsReceived || 0, seconds: r.totalSamplesDuration || 0,
    level: r.audioLevel || 0, energy: r.totalAudioEnergy || 0 });
})()`;

const listen = async (secs) => {
  const before = await says(SOUND_STATS);
  await sleep(secs * 1000);
  const after = await says(SOUND_STATS);
  if (typeof before !== 'string' || !before.startsWith('{')) return before;
  if (typeof after !== 'string' || !after.startsWith('{')) return after;
  const a = JSON.parse(before), b = JSON.parse(after);
  return JSON.stringify({
    packets: b.packets - a.packets,
    seconds: +(b.seconds - a.seconds).toFixed(2),
    loudest: +(b.level || 0).toFixed(4),
    energy: +((b.energy - a.energy) || 0).toFixed(4),
  });
};

console.log('\nwith the page playing a tone:');
console.log(' ', await listen(Math.round(SECONDS / 2)));

console.log('\nwith the page silent (and another browser playing the same tone):');
level = 0;
const other = spawn(process.env.CHROME
  || path.join(process.env.PROGRAMFILES, 'Google', 'Chrome', 'Application', 'chrome.exe'), [
  '--user-data-dir=' + fs.mkdtempSync(path.join(os.tmpdir(), 'other-')),
  '--no-first-run', '--autoplay-policy=no-user-gesture-required',
  '--app=data:text/html,' + encodeURIComponent(
    `<script>const c=new AudioContext();const o=c.createOscillator();const g=c.createGain();` +
    `o.frequency.value=440;g.gain.value=0.3;o.connect(g).connect(c.destination);o.start();c.resume()</script>`),
], { stdio: 'ignore' });
await sleep(3000);
console.log(' ', await listen(Math.round(SECONDS / 2)));

cdp.close();
chrome.kill();
other.kill();
server.close();
stopApp();
await sleep(400);
try { fs.rmSync(profile, { recursive: true, force: true }); } catch {}
