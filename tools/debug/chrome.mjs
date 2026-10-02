/** Shared discovery and CDP transport for isolated browser checks. */
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import {spawn} from 'node:child_process';

export function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  const guesses = process.platform === 'win32'
    ? [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA]
      .filter(Boolean).map(base => path.join(base, 'Google/Chrome/Application/chrome.exe'))
    : process.platform === 'darwin'
      ? ['/Applications/Google Chrome.app/Contents/MacOS/Google Chrome']
      : ['/usr/bin/google-chrome', '/usr/bin/chromium', '/usr/bin/chromium-browser'];
  const found = guesses.find(p => fs.existsSync(p));
  if (!found) throw new Error('Chrome was not found; set CHROME to its path');
  return found;
}

export function findCargo() {
  const beside = path.join(os.homedir(), '.cargo', 'bin', process.platform === 'win32' ? 'cargo.exe' : 'cargo');
  return fs.existsSync(beside) ? beside : 'cargo';
}

export async function connectCdp(target, {timeout = 30000} = {}) {
  const ws = new WebSocket(typeof target === 'string' ? target : target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { ws.close(); reject(new Error('CDP connection timed out')); }, timeout);
    ws.addEventListener('open', () => { clearTimeout(timer); resolve(); }, {once:true});
    ws.addEventListener('error', () => { clearTimeout(timer); reject(new Error('CDP connection failed')); }, {once:true});
  });
  let id = 0;
  let closed = false;
  const waiting = new Map(), thrown = [];
  const fail = () => {
    closed = true;
    for (const {reject, timer} of waiting.values()) { clearTimeout(timer); reject(new Error('CDP connection closed')); }
    waiting.clear();
  };
  ws.addEventListener('close', fail);
  ws.addEventListener('error', fail);
  ws.addEventListener('message', event => {
    const message = JSON.parse(event.data);
    const pending = waiting.get(message.id);
    if (pending) {
      clearTimeout(pending.timer); waiting.delete(message.id);
      if (message.error) pending.reject(new Error(pending.method + ': ' + JSON.stringify(message.error)));
      else pending.resolve(message.result);
    } else if (message.method === 'Runtime.exceptionThrown') {
      const d = message.params.exceptionDetails;
      thrown.push(d.exception?.description || d.exception?.value || d.text);
    }
  });
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    if (closed) { reject(new Error('CDP connection closed')); return; }
    const n = ++id;
    const timer = setTimeout(() => { waiting.delete(n); reject(new Error(method + ' timed out')); }, timeout);
    waiting.set(n, {resolve, reject, timer, method});
    try { ws.send(JSON.stringify({id:n, method, params})); }
    catch (error) { clearTimeout(timer); waiting.delete(n); reject(error); }
  });
  const run = async expression => {
    const r = await send('Runtime.evaluate', {expression, returnByValue:true, awaitPromise:true});
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  };
  return {ws, send, run, thrown, stop:() => ws.close()};
}

/** Port and profile belong to this process; concurrent checks cannot attach
 * to each other's Chrome, even when started from the same worktree. */
export async function startChrome({chrome = findChrome(), args = []} = {}) {
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'shikisha-chrome-'));
  const proc = spawn(chrome, ['--headless=new', '--remote-debugging-port=0',
    '--user-data-dir=' + profile, '--no-first-run', '--no-default-browser-check',
    '--hide-scrollbars', ...args, 'about:blank'], {stdio:'ignore', windowsHide:true});
  let spawnError;
  proc.once('error', e => { spawnError = e; });
  const stop = () => {
    proc.kill();
    // This directory was created by this invocation, never supplied by a caller.
    if (path.dirname(profile) !== path.resolve(os.tmpdir()) || !path.basename(profile).startsWith('shikisha-chrome-')) return;
    const clean = () => fs.promises.rm(profile, {recursive:true, force:true, maxRetries:5, retryDelay:200}).catch(() => {});
    if (proc.exitCode !== null || spawnError) clean(); else proc.once('exit', clean);
  };
  try {
    const deadline = Date.now() + 20000;
    while (Date.now() < deadline) {
      if (spawnError) throw spawnError;
      if (proc.exitCode !== null) throw new Error('Chrome exited before opening its debugging port');
      const active = path.join(profile, 'DevToolsActivePort');
      if (fs.existsSync(active)) {
        const port = Number(fs.readFileSync(active, 'utf8').split(/\r?\n/)[0]);
        const list = await fetch(`http://127.0.0.1:${port}/json/list`).then(r => r.json()).catch(() => []);
        const target = list.find(t => t.type === 'page');
        if (target) {
          const connection = await connectCdp(target);
          return {...connection, stop:() => { connection.stop(); stop(); }};
        }
      }
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    throw new Error('Chrome never opened its debugging port');
  } catch (error) { stop(); throw error; }
}
