/**
 * Does the sound of the watched page reach the far end, and nothing else's?
 *
 *     Windows: cargo build --release --bin SHIKISHA-TERM
 *     Mac:     cargo build, then the .app (tools/debug/lib/copy.mjs says how)
 *     node tools/debug/relay-sound.mjs [--seconds 20]
 *
 * Needs Windows or a Mac (macOS 14.2 or later), Chrome and Node. Its own copy
 * of the app, its own folder, its own port; nothing of a copy somebody is
 * using is read or stopped. Set CHROME where Chrome is not where it usually is.
 *
 * What it does. The copy is pointed at a page that plays a steady tone. A
 * headless Chrome stands in for the phone, opens the relay, waits for the
 * video to carry the picture, then taps the speaker the way a finger would --
 * and reads the browser's own figures for the sound it is receiving: how many
 * packets, how many seconds of it, and how loud.
 *
 * Then the same again with the tone stopped, because "we hear something" is
 * only half of it. The other half is that a page playing nothing sends
 * nothing, and that nothing else on this machine is heard: the tone is played
 * by ANOTHER program for the second half (a second Chrome on Windows, afplay
 * on a Mac), and what the phone hears then must be silence.
 *
 * A Mac asks the person, the first time, whether the app may record what other
 * apps play. The question comes up while the first half is listening; answer
 * it, and the figures say what the answer did. Until it is answered, and after
 * a "Don't Allow", what reaches the phone is silence.
 */
import { findChrome, connectCdp } from './chrome.mjs';
import { startCopy, tmp } from './lib/copy.mjs';
import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import { spawn } from 'node:child_process';

const RUN = tmp('sk-sound');
const RELAY_PORT = 8797;
const PAGE_PORT = 8904;
const PHONE_CDP = 9355;
const TOKEN = 'sound-token-0123456789abcdef';
const SECONDS = Number((process.argv[process.argv.indexOf('--seconds') + 1]) || 20);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };

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
console.log('starting a copy of this build, isolated');
const copy = await startCopy({
  run: RUN,
  release: true,
  // The page plays as it opens. Nobody is there to touch it first, and a
  // browser keeps a page quiet until somebody has
  chromiumArgs: ['--autoplay-policy=no-user-gesture-required'],
  config: {
    language: 'ja',
    remote: { enabled: true, bind: '127.0.0.1', port: RELAY_PORT, sticky_token: true, fixed_token: TOKEN },
    agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
    desks: [{ name: 'Sound', id: 'sound', browsers: [{ id: 'tone', url: `http://127.0.0.1:${PAGE_PORT}/` }] }],
  },
}).catch((e) => die(e.message));
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
  // The relay answers before the board has been drawn the first time, and
  // until then its state has no tabs in it
  let state, tab;
  for (let i = 0; i < 120 && !tab; i++) {
    state = await (await fetch(`${door}/api/state?t=${TOKEN}`, { headers: { cookie } })).json();
    // The rail's own list: terminals and browsers together, which is what a
    // finger picks from
    const tabs = (state.ui && state.ui.tabs) || state.tabs || [];
    tab = tabs.find((t) => t.name === 'tone' || t.id === 'tone');
    if (!tab) await sleep(250);
  }
  if (!tab) { copy.stop(); die('the copy has no tab called tone: ' + JSON.stringify(state).slice(0, 200)); }
  await fetch(`${door}/api/intent?t=${TOKEN}`, {
    method: 'POST',
    headers: { cookie, 'content-type': 'application/json' },
    body: JSON.stringify({ kind: 'select', tab: tab.index }),
  });
  console.log('the tone tab is the one on screen');
  await sleep(1500);
}

// ── The viewer ────────────────────────────────
const profile = fs.mkdtempSync(tmp('sound-'));
const chrome = spawn(findChrome(), [
  '--headless=new', '--remote-debugging-port=' + PHONE_CDP, '--user-data-dir=' + profile,
  '--window-size=412,915', '--autoplay-policy=no-user-gesture-required', '--use-mock-keychain',
  '--no-first-run', '--no-default-browser-check', 'about:blank',
], { stdio: 'ignore' });
let page;
for (let i = 0; i < 80 && !page; i++) {
  try { page = (await (await fetch(`http://127.0.0.1:${PHONE_CDP}/json/list`)).json()).find((t) => t.type === 'page'); } catch {}
  if (!page) await sleep(250);
}
if (!page) { copy.stop(); die('the stand-in phone did not start'); }
const cdp = await connectCdp(page);
await cdp.send('Page.enable');
await cdp.send('Runtime.enable');
await cdp.send('Emulation.setDeviceMetricsOverride', { width: 412, height: 915, deviceScaleFactor: 2, mobile: true });
await cdp.send('Page.navigate', { url: `http://127.0.0.1:${RELAY_PORT}/?t=${TOKEN}` });
// Long enough for the picture to start and for video to take over from the
// line it opened first
await sleep(14000);

const says = async (expression) => {
  const m = await cdp.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
  if (m.exceptionDetails) return 'the page threw: ' + JSON.stringify(m.exceptionDetails).slice(0, 200);
  const r = m.result;
  if (!r) return null;
  return typeof r.value === 'string' || typeof r.value === 'number' || r.value == null ? r.value : JSON.stringify(r.value);
};

console.log('the picture comes as:', await says(`(() => { const w = document.getElementById('castway'); return (typeof videoOn !== 'undefined' && videoOn ? 'video' : 'stills') + ' (' + (w ? w.textContent : '-') + ')'; })()`));

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

console.log('\nwith the page silent (and another program playing the same tone):');
level = 0;
const other = otherPlayer();
await sleep(3000);
console.log(' ', await listen(Math.round(SECONDS / 2)));

// What the copy itself said about the sound
try {
  const said = fs.readFileSync(path.join(copy.logs, 'hooks.log'), 'utf8').split('\n').filter((l) => /sound/.test(l));
  console.log('\nthe copy said:\n  ' + (said.join('\n  ') || '(nothing about the sound)'));
} catch {}

cdp.ws.close();
chrome.kill();
other.kill();
server.close();
copy.stop();
await sleep(400);
try { fs.rmSync(profile, { recursive: true, force: true }); } catch {}

/** The same tone, played by a program that is not the copy */
function otherPlayer() {
  if (process.platform === 'darwin') {
    // Ten seconds of it longer than the half lasts, as a file afplay plays
    const rate = 48000, n = rate * (SECONDS + 10);
    const wav = Buffer.alloc(44 + n * 2);
    wav.write('RIFF', 0); wav.writeUInt32LE(36 + n * 2, 4); wav.write('WAVEfmt ', 8);
    wav.writeUInt32LE(16, 16); wav.writeUInt16LE(1, 20); wav.writeUInt16LE(1, 22);
    wav.writeUInt32LE(rate, 24); wav.writeUInt32LE(rate * 2, 28); wav.writeUInt16LE(2, 32); wav.writeUInt16LE(16, 34);
    wav.write('data', 36); wav.writeUInt32LE(n * 2, 40);
    for (let i = 0; i < n; i++) wav.writeInt16LE(Math.round(0.3 * 32767 * Math.sin(2 * Math.PI * 440 * i / rate)), 44 + i * 2);
    const file = path.join(profile, 'tone.wav');
    fs.writeFileSync(file, wav);
    return spawn('afplay', [file], { stdio: 'ignore' });
  }
  return spawn(findChrome(), [
    '--user-data-dir=' + fs.mkdtempSync(tmp('other-')),
    '--no-first-run', '--autoplay-policy=no-user-gesture-required',
    '--app=data:text/html,' + encodeURIComponent(
      `<script>const c=new AudioContext();const o=c.createOscillator();const g=c.createGain();` +
      `o.frequency.value=440;g.gain.value=0.3;o.connect(g).connect(c.destination);o.start();c.resume()</script>`),
  ], { stdio: 'ignore' });
}
