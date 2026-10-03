/**
 * A relayed browser page must lay out at the viewer's width, including when
 * joining, rotating, changing panes and switching pages. Disconnecting must
 * restore its desktop size, and a press must still land at the right place.
 *
 * Build the app, then run `node tools/debug/browser-width.win.mjs` on Windows
 * with Chrome installed. --jpeg checks the still-picture fallback; --split
 * checks pages rendered by the separate runtime's Chrome. --ja uses Japanese.
 * Uses instance.win.ps1 with a fresh folder; no running user's app is touched.
 * Pictures and measurements are written to target/shots.
 */
import {connectCdp, startChrome} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import http from 'node:http';
import {spawnSync} from 'node:child_process';
import {randomUUID} from 'node:crypto';

const root = path.resolve(import.meta.dirname, '../..');
const lab = fs.mkdtempSync(path.join(os.tmpdir(), 'sk-browser-width-'));
const instance = path.join(lab, 'instance');
const launch = path.join(root, 'tools/debug/instance.win.ps1');
const jpeg = process.argv.includes('--jpeg'), split = process.argv.includes('--split');
const language = process.argv.includes('--ja') ? 'ja' : 'en';
const token = randomUUID();
const shots = path.join(root, 'target/shots');
fs.mkdirSync(shots, {recursive:true});
const prefix = `browser-width-${split ? 'split' : 'window'}-${jpeg ? 'jpeg' : 'video'}-${language}`;
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const quote = value => "'" + value.replaceAll("'", "''") + "'";
const ps = command => {
  const r = spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', command],
    {encoding:'utf8', windowsHide:true});
  if (r.status !== 0) throw new Error(r.stdout + r.stderr);
  return r.stdout;
};
async function until(f, what, ms = 20000) {
  const end = Date.now() + ms;
  let last;
  while (Date.now() < end) {
    try { last = await f(); if (last) return last; } catch (e) { last = e.message; }
    await sleep(150);
  }
  throw new Error(`Timeout: ${what} (${JSON.stringify(last)})`);
}
const measured = new Map(), pressed = [];
const server = http.createServer((req, res) => {
  const url = new URL(req.url, 'http://127.0.0.1');
  if (url.pathname === '/measure') {
    measured.set(url.searchParams.get('page'), JSON.parse(url.searchParams.get('size')));
    res.writeHead(204).end(); return;
  }
  if (url.pathname === '/press') { pressed.push(url.searchParams.get('page')); res.writeHead(204).end(); return; }
  res.writeHead(200, {'content-type':'text/html; charset=utf-8', 'cache-control':'no-store'});
  res.end(`<!doctype html><meta name="viewport" content="width=device-width,initial-scale=1">
    <title>Responsive page</title><style>
    body{font:18px system-ui;margin:24px}main{display:grid;grid-template-columns:1fr 1fr;gap:24px}
    @media(max-width:600px){main{grid-template-columns:1fr}}
    button{position:fixed;right:24px;bottom:24px;font:inherit;padding:16px}
    </style><h1>Responsive page</h1><p id="size"></p><main><article>First column</article><article>Second column</article></main>
    <button onclick="fetch('/press?page='+location.pathname)">Press here</button><script>
    function measure(){const size={w:innerWidth,h:innerHeight,dpr:devicePixelRatio,
      columns:getComputedStyle(document.querySelector('main')).gridTemplateColumns.split(' ').length};
      document.getElementById('size').textContent=size.w+' × '+size.h+' CSS pixels';
      fetch('/measure?page='+location.pathname+'&size='+encodeURIComponent(JSON.stringify(size)));}
    addEventListener('resize',measure);measure();</script>`);
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const pageUrl = `http://127.0.0.1:${server.address().port}`;
const config = path.join(lab, 'config.json');
fs.writeFileSync(config, JSON.stringify({language, split, resident:false,
  agent_hooks:{'Claude Code':'off', 'Codex CLI':'off', 'Gemini CLI':'off'},
  remote:{sticky_token:true, fixed_token:token},
  desks:[{id:'check', name:'Browser width', folders:[{cwd:'{work}', tabs:[
    {id:'first', name:'First', command:`browser ${pageUrl}/first`},
    {id:'second', name:'Second', command:`browser ${pageUrl}/second`},
  ]}]}],
}));
let local, phone, board;
const records = [];
const note = (what, detail) => { records.push({what,...detail}); console.log(what, JSON.stringify(detail)); };
const select = (client, id) => client.run(`send({kind:"select",tab:S.tabs.find(t => t.id === ${JSON.stringify(id)}).index})`);
const shape = () => phone.run(`(() => {const c=document.getElementById('cast'),v=document.getElementById('castv');
  return {w:c.clientWidth,h:c.clientHeight,video:videoOn,pixels:videoOn?v.videoWidth:c.width}})()`);
async function fitted(page, what) {
  await until(async () => {
    const want = await shape(), got = measured.get(page);
    return got && Math.abs(got.w - want.w) <= 2 && Math.abs(got.h - want.h) <= 2
      && Math.abs(want.pixels - got.w * got.dpr) <= 3;
  }, what);
  note(what, {viewer:await shape(), page:measured.get(page)});
}
async function shot(name) {
  const {data} = await phone.send('Page.captureScreenshot', {format:'png'});
  fs.writeFileSync(path.join(shots, `${prefix}-${name}.png`), Buffer.from(data, 'base64'));
}
try {
  const started = ps(`& ${quote(launch)} -At ${quote(instance)} -Config ${quote(config)}`);
  board = /^board=(.+)$/m.exec(started)[1].trim();
  const cdp = /^cdp=(.+)$/m.exec(started)[1].trim();
  const target = await until(async () => (await (await fetch(cdp+'/json/list')).json()).find(t=>t.type==='page'), 'local board');
  local = await connectCdp(target);
  await until(() => local.run('typeof S !== "undefined" && S?.tabs.some(t=>t.id === "first")'), 'browser tab');
  await select(local, 'first');
  await until(() => measured.get('/first')?.w > 600, 'desktop page');
  // The page first opens in a temporary surface; let the board place it in
  // the visible pane before remembering what disconnect should restore.
  await sleep(1200);
  const desktop = measured.get('/first');
  note('desktop', desktop);
  phone = await startChrome();
  await phone.send('Page.enable');
  if (jpeg) await phone.send('Page.addScriptToEvaluateOnNewDocument', {source:'window.RTCPeerConnection = undefined;'});
  await phone.send('Emulation.setDeviceMetricsOverride', {width:390,height:844,deviceScaleFactor:3,mobile:true});
  await phone.send('Page.navigate', {url:board+'/?t='+token});
  await until(() => phone.run('typeof S !== "undefined" && S?.tabs.some(t=>t.id === "first")'), 'remote board');
  await select(phone, 'first');
  await fitted('/first', 'joining fits the browser page');
  if (measured.get('/first').columns !== 1) throw new Error('The narrow page did not reflow to one column');
  if (!jpeg) await until(() => phone.run('videoOn'), 'video transport');
  await fitted('/first', 'the transport keeps the width');
  if (jpeg && await phone.run('videoOn')) throw new Error('The JPEG test opened a video connection');
  await shot('portrait');
  // The bottom-right button must receive a press after the viewport and the
  // image have been scaled for a dense screen.
  await phone.run(`(() => {const c=document.getElementById('cast');
    const point={x:1-65/c.clientWidth,y:1-50/c.clientHeight};
    sendIn({kind:'inject',what:'mouse',phase:'pressed',...point,down:true});
    sendIn({kind:'inject',what:'mouse',phase:'released',...point,down:false});})()`);
  await until(() => pressed.includes('/first'), 'the press reaches the resized page');
  note('press reaches the page', {pressed});
  await phone.send('Emulation.setDeviceMetricsOverride', {width:844,height:390,deviceScaleFactor:3,mobile:true});
  await fitted('/first', 'rotation fits the browser page');
  await shot('landscape');
  // No window resize: a pane getting narrower must also report its new width.
  await phone.run('document.getElementById("main").style.width="300px"');
  await fitted('/first', 'a changed content area fits the browser page');
  await phone.run('document.getElementById("main").style.width=""');
  await phone.send('Emulation.setDeviceMetricsOverride', {width:390,height:844,deviceScaleFactor:3,mobile:true});
  await fitted('/first', 'portrait is restored');
  await select(phone, 'second');
  await fitted('/second', 'switching browser tabs fits the new page');
  await select(phone, 'first');
  await fitted('/first', 'switching back fits the first page');
  await phone.send('Page.navigate', {url:'about:blank'});
  await until(() => Math.abs(measured.get('/first')?.w-desktop.w)<=2, 'disconnect restores desktop width');
  note('disconnect restores desktop width', measured.get('/first'));
  await phone.send('Page.navigate', {url:board+'/?t='+token});
  await fitted('/first', 'reconnecting fits the browser page');
  if (split) {
    // DevTools is another browser tab, with its own responsive layout.
    await phone.run('send({kind:"devtools",page:"first"})');
    await until(() => phone.run('S.tabs.some(t=>t.id === "first-devtools")'), 'DevTools tab');
    await select(phone, 'first-devtools');
    const portFile = path.join(instance, 'localappdata/ShikishaTerm/chromium/default/DevToolsActivePort');
    const port = Number(fs.readFileSync(portFile, 'utf8').split(/\r?\n/)[0]);
    const target = await until(async () => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json())
      .find(t=>t.url.startsWith('devtools://')), 'DevTools page');
    const dev = await connectCdp(target);
    try {
      await until(async () => Math.abs(await dev.run('innerWidth')-(await shape()).w)<=2, 'DevTools fits the phone');
      note('DevTools fits the phone', {width:await dev.run('innerWidth'),viewer:await shape()});
      await shot('devtools');
    } finally { dev.stop(); }
  }
  console.log('all passed');
} catch (error) {
  console.error(error.message, JSON.stringify({measured:[...measured], viewer:phone ? await shape().catch(()=>null) : null}));
  if (phone) await shot('failure').catch(()=>{});
  process.exitCode = 1;
} finally {
  fs.writeFileSync(path.join(shots, `${prefix}.json`), JSON.stringify(records,null,2));
  phone?.stop(); local?.stop();
  ps(`& ${quote(launch)} -At ${quote(instance)} -Stop`);
  server.close();
}
