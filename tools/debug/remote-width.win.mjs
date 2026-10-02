/**
 * Check the terminal width when a phone connects after somebody used the PC.
 * Run `cargo build`, then `node tools/debug/remote-width.win.mjs` on Windows
 * with Chrome installed; add --poll to check the HTTP fallback as well.
 * Uses instance.win.ps1 and a fresh temporary folder;
 * never touches a running user's app. No AI account is used: ruler-tui reports
 * the dimensions the real pseudo terminal gives a full-screen program.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';

const root = path.resolve(import.meta.dirname, '../..');
const lab = fs.mkdtempSync(path.join(os.tmpdir(), 'sk-remote-width-'));
const instance = path.join(lab, 'instance');
const token = randomUUID();
const polling = process.argv.includes('--poll');
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const quote = value => "'" + value.replaceAll("'", "''") + "'";
const ps = command => {
  const r = spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', command], {encoding: 'utf8'});
  if (r.status !== 0) throw new Error(r.stdout + r.stderr);
  return r.stdout;
};
const launch = path.join(root, 'tools/debug/instance.win.ps1');
const config = path.join(lab, 'config.json');
fs.writeFileSync(config, JSON.stringify({
  language: 'en', resident: false,
  agent_hooks: {'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off'},
  remote: {sticky_token: true, fixed_token: token},
  desks: [{id: 'check', name: 'Check', folders: [{cwd: '{work}', tabs: [
    {id: 'ruler', name: 'Ruler', command: [process.execPath, path.join(root, 'tools/debug/ruler-tui.mjs')]},
  ]}]}],
}));
async function until(f, what, ms = 15000) {
  const end = Date.now() + ms;
  let last;
  while (Date.now() < end) {
    try { last = await f(); if (last) return last; } catch (e) { last = e.message; }
    await sleep(200);
  }
  throw new Error(`Timeout: ${what} (${JSON.stringify(last)})`);
}
async function attach(port) {
  const target = await until(async () => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json())
    .find(t => t.type === 'page'), 'DevTools page');
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
  let seq = 0;
  const pending = new Map();
  ws.onmessage = e => {
    const m = JSON.parse(e.data);
    const item = pending.get(m.id);
    if (!item) return;
    pending.delete(m.id);
    clearTimeout(item.timer);
    if (m.error) item.reject(new Error(JSON.stringify(m.error))); else item.resolve(m.result);
  };
  const call = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++seq;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`Timeout: ${method}`)); }, 15000);
    pending.set(id, {resolve, reject, timer});
    ws.send(JSON.stringify({id, method, params}));
  });
  const run = async expression => {
    const r = await call('Runtime.evaluate', {expression, returnByValue: true, awaitPromise: true});
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  };
  return {call, run, close: () => ws.close()};
}
const look = `(() => {
  const screen = document.getElementById('screen');
  const said = /C=(\\d+) R=(\\d+) D=(\\d+)/.exec(screen.innerText);
  const rows = screen.innerText.split('\\n').filter(l => /^R\\d+/.test(l));
  return {cols: said ? +said[1] : 0, expected: lastFit?.cols,
    drawn: said ? +said[3] : 0, widths: [...new Set(rows.map(l => l.trimEnd().length))]};
})()`;
let chrome, local, phone;
let failed = 0;
const check = (ok, text, detail) => {
  console.log(`${ok ? 'PASS' : 'FAIL'} ${text} ${JSON.stringify(detail ?? '')}`);
  if (!ok) failed++;
};
try {
  const started = ps(`$PSDefaultParameterValues['Start-Process:WindowStyle']='Hidden'; & ${quote(launch)} -At ${quote(instance)} -Config ${quote(config)}`);
  const board = /^board=(.+)$/m.exec(started)[1].trim();
  const localPort = +/^cdp=http:\/\/127\.0\.0\.1:(\d+)/m.exec(started)[1];
  local = await attach(localPort);
  await until(() => local.run('typeof S !== "undefined" && S && S.tabs.length'), 'window state');
  await local.run('send({kind:"select",tab:1})');
  await until(async () => (await local.run(look)).cols > 0, 'ruler screen');
  // A real UI key through the local window's IPC, before a remote viewer exists.
  const original = await local.run(look);
  await local.run('send({kind:"key",text:"x"})');
  await until(async () => (await local.run(look)).drawn > original.drawn, 'local key reaches the terminal');
  await sleep(500);
  const before = await local.run(look);
  console.log('PC before connection', JSON.stringify(before));
  const chromeExe = [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA]
    .filter(Boolean).map(p => path.join(p, 'Google/Chrome/Application/chrome.exe')).find(p => fs.existsSync(p));
  if (!chromeExe) throw new Error('Chrome is required');
  const profile = path.join(lab, 'chrome');
  chrome = spawn(chromeExe, ['--headless=new', '--remote-debugging-port=0', '--user-data-dir=' + profile,
    '--no-first-run', 'about:blank'], {stdio: 'ignore', windowsHide: true});
  const chromePort = await until(() => fs.existsSync(path.join(profile, 'DevToolsActivePort'))
    && +fs.readFileSync(path.join(profile, 'DevToolsActivePort'), 'utf8').split('\n')[0], 'Chrome port');
  phone = await attach(chromePort);
  await phone.call('Page.enable');
  if (polling) await phone.call('Page.addScriptToEvaluateOnNewDocument', {
    source: 'window.WebSocket = class { constructor() { throw new Error("Testing the HTTP fallback"); } };',
  });
  await phone.call('Emulation.setDeviceMetricsOverride', {width: 390, height: 844, deviceScaleFactor: 1, mobile: false});
  await phone.call('Page.navigate', {url: board + '/?t=' + token});
  await until(async () => (await phone.run(look)).cols > 0, 'phone screen');
  await sleep(2000);
  const joined = await phone.run(look);
  check(joined.cols === joined.expected && joined.cols < before.cols, 'joining after PC input fits the phone without typing', joined);
  const shot = await phone.call('Page.captureScreenshot', {format: 'png'});
  const shots = path.join(root, 'target/shots');
  fs.mkdirSync(shots, {recursive: true});
  fs.writeFileSync(path.join(shots, `remote-width-${polling ? 'poll' : 'socket'}.png`), Buffer.from(shot.data, 'base64'));
  await phone.run('send({kind:"key",text:"x"})');
  await sleep(1500);
  const typed = await phone.run(look);
  check(typed.cols === typed.expected, 'typing on the phone fits the phone', typed);
  await phone.call('Emulation.setDeviceMetricsOverride', {width: 780, height: 500, deviceScaleFactor: 1, mobile: false});
  await sleep(2000);
  const turned = await phone.run(look);
  check(turned.cols === turned.expected && turned.cols > typed.cols,
    'turning the remote screen resizes the running terminal', turned);
  await phone.call('Emulation.setDeviceMetricsOverride', {width: 390, height: 844, deviceScaleFactor: 1, mobile: false});
  await sleep(2000);
  await local.run('send({kind:"key",text:"x"})');
  await sleep(1500);
  const back = await local.run(look);
  check(back.cols === back.expected && back.cols === before.cols, 'PC input takes its width back', back);
  await phone.run('lastRC = ""; report()');
  await sleep(1500);
  check((await local.run(look)).cols === back.cols, 'a passive remote repaint does not take width from the PC');
  if (!polling) {
    await phone.call('Page.reload');
    await until(async () => (await phone.run(look)).cols === typed.cols, 'reloading hands the width to the phone');
    check(true, 'reload takes over while the old socket may still be present');
  }
  await phone.call('Page.navigate', {url: 'about:blank'});
  await sleep(8500);
  await local.run('send({kind:"key",text:"x"})');
  await phone.call('Page.navigate', {url: board + '/?t=' + token});
  await until(async () => (await phone.run(look)).cols > 0, 'reconnected phone');
  await sleep(2000);
  const again = await phone.run(look);
  check(again.cols === again.expected, 'connecting again fits the phone', again);
  await phone.call('Page.navigate', {url: 'about:blank'});
  await until(async () => (await local.run(look)).cols === before.cols, 'disconnect returns the terminal width to the PC');
  check(true, 'disconnect restores PC width without a keystroke');
} finally {
  phone?.close(); local?.close(); chrome?.kill();
  // This directory was freshly made above; the helper stops only its copy.
  ps(`& ${quote(launch)} -At ${quote(instance)} -Stop`);
}
process.exitCode = failed ? 1 : 0;
