/**
 * Does the screen actually reach a browser as video?
 *
 *     node tools/debug/video-endtoend.mjs [seconds]
 *
 * Every piece of the path has tests of its own. What they cannot answer is
 * whether a browser decodes what this machine sends: between the two ends are
 * a codec agreement, a packetiser, an encoder written against an operating
 * system's interface, and the browser's own idea of what it will play. Each
 * can be right alone and wrong together, and the failure looks like a black
 * rectangle with nothing in any log.
 *
 * Three steps:
 *
 *  1. Chrome draws a moving page and its own screen relay is read for real
 *     JPEGs -- the same command and the same settings the app uses
 *     (`crates/core/src/cdp.rs` CAST_PARAMS), so what is fed in is the shape
 *     the product actually gets
 *  2. `cargo run --bin video_probe` stands the real relay up and pushes those
 *     pictures through the product's own door
 *  3. A second Chrome opens the relay's own page -- same origin, so it may
 *     talk to it -- and asks for the picture as video. What it reports is how
 *     many frames it decoded
 *
 * Needs Chrome and a built workspace. Nothing here ships.
 */
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { attach } from '../../.private/doc/proto/webrtc-http/cdp.mjs';

const SECS = Number(process.argv[2] || 20);
const ROOT = path.resolve(import.meta.dirname, '..', '..');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const say = (s) => console.log(s);

function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  const guesses = [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA]
    .filter(Boolean)
    .map((base) => path.join(base, 'Google/Chrome/Application/chrome.exe'));
  for (const g of guesses) if (fs.existsSync(g)) return g;
  throw new Error('no Chrome found; set CHROME');
}
const CHROME = findChrome();

const PAGE = `<!doctype html><meta charset="utf-8"><title>moving</title>
<style>html,body{margin:0;background:#101014;overflow:hidden;height:100%}
.bar{position:absolute;top:0;width:220px;height:200px;background:#4aa3ff;animation:s 2s linear infinite}
.bar2{position:absolute;left:0;width:180px;height:150px;background:#19c37d;animation:d 3s linear infinite}
@keyframes s{from{transform:translateX(0)}to{transform:translateX(1000px)}}
@keyframes d{from{transform:translateY(0)}to{transform:translateY(500px)}}</style>
<div class="bar"></div><div class="bar2"></div>`;

const work = fs.mkdtempSync(path.join(os.tmpdir(), 'videoprobe-'));
const shots = path.join(work, 'shots');
fs.mkdirSync(shots);
fs.writeFileSync(path.join(work, 'anim.html'), PAGE);
const kids = [];
const done = () => {
  for (const k of kids) { try { k.kill(); } catch {} }
  try { fs.rmSync(work, { recursive: true, force: true }); } catch {}
};

// ── 1. Real JPEGs, from the same command the app uses ────────────────
say('collecting pictures the way the app gets them…');
const drawer = spawn(CHROME, [
  `--user-data-dir=${path.join(work, 'draw')}`,
  '--no-first-run', '--no-default-browser-check',
  '--remote-debugging-port=9334', '--remote-allow-origins=*',
  '--window-size=960,600', '--window-position=20,20',
  `file://${path.join(work, 'anim.html').replace(/\\/g, '/')}`,
], { stdio: 'ignore' });
kids.push(drawer);
await sleep(4000);

const cdp = await attach(9334);
let got = 0;
cdp.on((m) => {
  if (m.method !== 'Page.screencastFrame' || got >= 90) return;
  fs.writeFileSync(
    path.join(shots, String(got).padStart(3, '0') + '.jpg'),
    Buffer.from(m.params.data, 'base64'),
  );
  got++;
  cdp.send({ id: 70000 + got, method: 'Page.screencastFrameAck',
    params: { sessionId: m.params.sessionId } });
});
await cdp.call('Page.enable');
await cdp.call('Page.startScreencast',
  { format: 'jpeg', quality: 60, maxWidth: 1600, maxHeight: 2400, everyNthFrame: 1 });
await sleep(3000);
await cdp.call('Page.stopScreencast');
cdp.close();
drawer.kill();
say(`  ${got} pictures`);
if (!got) { say('nothing was captured; nothing to send'); done(); process.exit(1); }

// ── 2. The product's own relay, fed those pictures ───────────────────
say('standing the relay up…');
const probe = spawn('cargo', ['run', '-q', '-p', 'shikisha-core', '--bin', 'video_probe', '--',
  shots, String(SECS + 10)], { cwd: ROOT, stdio: ['ignore', 'pipe', 'pipe'] });
kids.push(probe);
let said = '';
probe.stdout.on('data', (d) => { said += d; });
probe.stderr.on('data', (d) => { said += d; });
const until = Date.now() + 180000;
while (!said.includes('ready') && Date.now() < until) await sleep(500);
if (!said.includes('ready')) { say('the relay never came up:\n' + said); done(); process.exit(1); }
const origin = (said.match(/^origin (.*)$/m) || [])[1];
const token = (said.match(/^token (.*)$/m) || [])[1];
const codec = (said.match(/^encoder (.*)$/m) || [])[1];
say(`  ${origin}, sending ${codec}`);

// ── 3. A browser asks for it as video ────────────────────────────────
// The relay's own page is opened, so the code below is same-origin and may
// talk to it. What is being measured is the machine's half, so the asking is
// written here rather than leaning on the page's own
const viewer = spawn(CHROME, [
  `--user-data-dir=${path.join(work, 'view')}`,
  '--no-first-run', '--no-default-browser-check',
  '--remote-debugging-port=9335', '--remote-allow-origins=*',
  '--autoplay-policy=no-user-gesture-required',
  '--window-size=700,560', '--window-position=700,20',
  `${origin}/?t=${token}`,
], { stdio: 'ignore' });
kids.push(viewer);
await sleep(4000);

const see = await attach(9335);
const ask = `(async () => {
  window.__probe = {state: "starting", frames: 0, codec: "", why: ""};
  try {
    const pc = new RTCPeerConnection({iceServers: []});
    pc.addTransceiver("video", {direction: "recvonly"});
    const v = document.createElement("video");
    v.autoplay = true; v.muted = true; v.playsInline = true;
    document.body.append(v);
    pc.ontrack = (e) => { v.srcObject = e.streams[0]; v.play().catch(() => {}); };
    pc.onconnectionstatechange = () => { window.__probe.state = pc.connectionState; };
    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    await new Promise((r) => {
      if (pc.iceGatheringState === "complete") return r();
      pc.onicegatheringstatechange = () => { if (pc.iceGatheringState === "complete") r(); };
      setTimeout(r, 2000);
    });
    const said = await (await fetch("/api/video", {
      method: "POST",
      headers: {"content-type": "application/json", "X-Token": ${JSON.stringify(token)}},
      body: JSON.stringify({offer: pc.localDescription.sdp}),
    })).json();
    if (!said.ok) { window.__probe.why = said.why || "refused"; return; }
    // Kept so a connection that never comes up can be read rather than guessed at
    window.__probe.answer = said.answer;
    window.__probe.mine = pc.localDescription.sdp;
    pc.onicecandidateerror = (e) => { window.__probe.icefail = String(e.errorText || e.errorCode); };
    await pc.setRemoteDescription({type: "answer", sdp: said.answer});
    setInterval(async () => {
      const s = await pc.getStats();
      s.forEach((r) => {
        if (r.type === "inbound-rtp" && r.kind === "video") window.__probe.frames = r.framesDecoded || 0;
        if (r.type === "codec" && r.mimeType) window.__probe.codec = r.mimeType;
      });
    }, 500);
  } catch (e) { window.__probe.why = String(e); }
})()`;
await see.call('Runtime.evaluate', { expression: ask, awaitPromise: false });

for (let i = 0; i < SECS; i++) {
  await sleep(1000);
  const r = await see.call('Runtime.evaluate',
    { expression: 'JSON.stringify(window.__probe)', returnByValue: true });
  const p = JSON.parse(r?.result?.result?.value || '{}');
  process.stdout.write(`\r  ${p.state || '?'}  ${p.frames || 0} frames  ${p.codec || ''}      `);
  if (p.why) { say(`\n  it did not start: ${p.why}`); break; }
}
say('');

const last = await see.call('Runtime.evaluate',
  { expression: 'JSON.stringify(window.__probe)', returnByValue: true });
const p = JSON.parse(last?.result?.result?.value || '{}');
say('');
say('─── what the browser got ───');
say(`connection   : ${p.state}`);
say(`frames       : ${p.frames}`);
say(`codec        : ${p.codec || '(none agreed)'}`);
if (p.why) say(`trouble      : ${p.why}`);
if (p.icefail) say(`ice trouble  : ${p.icefail}`);
if (p.frames === 0 && p.answer) {
  const lines = (sdp) => String(sdp || '').split(/\r?\n/);
  say('');
  say('--- what this machine answered ---');
  for (const l of lines(p.answer)) if (keep.test(l)) say('  ' + l);
  say('--- what the browser offered ---');
  for (const l of lines(p.mine)) if (keep.test(l)) say('  ' + l);
}

say('');
say(p.frames > 0
  ? 'YES: the screen reached a browser as video, decoded frame by frame'
  : 'NO: nothing decoded. The connection state above says how far it got');

see.close();
done();
process.exit(p.frames > 0 ? 0 : 1);
