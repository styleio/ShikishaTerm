/**
 * A picture of AIConfer in a running copy of the app, through its board page:
 * the conference as the app itself serves it, with what real AIs said.
 *
 *     node tools/debug/confer-shot.mjs <board url> <out.png> [--phone] [--lang=ja]
 *
 * The board url is what confer-real.win.mjs --keep prints. The page is opened
 * in a headless Chrome, the conversation panel is turned to AIConfer, and the
 * screen is photographed once the conference has been read.
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';

const [url, out] = process.argv.slice(2);
if (!url || !out) { console.error('usage: confer-shot.mjs <board url> <out.png> [--phone]'); process.exit(2); }
const phone = process.argv.includes('--phone');
const [w, h] = phone ? [390, 820] : [1280, 860];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const chrome = [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA]
  .filter(Boolean).map((b) => path.join(b, 'Google/Chrome/Application/chrome.exe')).find((p) => fs.existsSync(p));
if (!chrome) { console.error('Chrome was not found'); process.exit(2); }
const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'confer-shot-'));
const port = 9300 + Math.floor(Math.random() * 90);
const browser = spawn(chrome, ['--headless=new', '--disable-gpu', `--remote-debugging-port=${port}`,
  `--user-data-dir=${profile}`, `--window-size=${w},${h}`, 'about:blank'], { stdio: 'ignore' });
try {
  let target;
  for (let i = 0; i < 40 && !target; i++) {
    await sleep(250);
    target = await fetch(`http://127.0.0.1:${port}/json`).then((r) => r.json()).then((l) => l.find((t) => t.type === 'page')).catch(() => null);
  }
  const {ws, send:cdp, run} = await connectCdp(target);
  await cdp('Emulation.setDeviceMetricsOverride', { width: w, height: h, deviceScaleFactor: 2, mobile: phone });
  await cdp('Page.navigate', { url });
  await sleep(6000);
  await run('if (!phoneWidth()) setSideWidth(420); sideReveal("convo"); convoModeTo("confer"); "ok"');
  await sleep(4000);
  const said = await run('JSON.stringify({n: CF.said.length, bad: CF.bad})');
  console.log('conference on the page:', said);
  const shot = await cdp('Page.captureScreenshot', { format: 'png' });
  fs.writeFileSync(out, Buffer.from(shot.data, 'base64'));
  console.log(out);
  ws.close();
} finally {
  browser.kill();
  await sleep(500);
  fs.rmSync(profile, { recursive: true, force: true });
}
