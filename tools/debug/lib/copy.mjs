/**
 * A copy of this checkout's build that is nobody's: a folder of its own, ports
 * of its own, and stopped by finding its own processes only -- a copy somebody
 * is using is neither read, written nor stopped.
 *
 *     const copy = await startCopy({ run, config, windowCdp })
 *     ... copy.home, copy.data, copy.logs, copy.cpuSeconds()
 *     copy.stop()
 *
 * `run` is the folder the copy lives in (emptied first), `config` what goes in
 * its config.json, `windowCdp` a DevTools port for the copy's own window (or
 * nothing), `chromiumArgs` more switches for the browser the window draws
 * with. `release` picks target/release over target/debug on Windows.
 *
 * Windows: the build is staged into `run`/app (tools/stage.ps1) and started
 * behind the other windows, with LOCALAPPDATA in `run` as well.
 *
 * Mac: the copy is the .app packaging/mac/mkapp.sh makes, named by SHIKISHA_APP
 * or else target/mac/SHIKISHA-TERM.app. Made with tools/debug/mkapp-test.mac.sh,
 * which signs it the same way every time, so that what the person at the Mac
 * allowed once (the sound, the screen) is not asked again for the next build:
 *
 *     sh tools/debug/mkapp-test.mac.sh target/debug/SHIKISHA-TERM \
 *         target/debug/shikisha-chromium-helper "$CEF" target/mac
 *
 * It is opened by Launch Services (`open -n`), not as a child of this script:
 * a Mac asks for a permission -- the camera, the sound other apps play -- in
 * the name of whoever started the program, and a child of a terminal would be
 * asked about as the terminal. Its folder is SHIKISHA_HOME, and its keychain a
 * pretend one: each build is a new signature for the real keychain to ask
 * about, and nobody is there to answer.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..', '..');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const ps = (...a) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...a], { encoding: 'utf8' });

// Nothing of an agent's session, nor of another copy's settings, is handed on
const cleanEnv = () => Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));

export function macApp() {
  return path.resolve(process.env.SHIKISHA_APP || path.join(ROOT, 'target', 'mac', 'SHIKISHA-TERM.app'));
}

export async function startCopy({ run, config, windowCdp = null, chromiumArgs = [], release = false }) {
  const args = [...(windowCdp ? [`--remote-debugging-port=${windowCdp}`] : []), ...chromiumArgs];
  return process.platform === 'darwin'
    ? startMac({ run, config, args })
    : startWindows({ run, config, args, release });
}

async function startWindows({ run, config, args, release }) {
  const app = path.join(run, 'app');
  const local = path.join(run, 'localappdata');
  const stop = () => ps('-Command',
    `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
    `Where-Object { $_.Path -and $_.Path -like '${run}\\*' } | ` +
    `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
  const exe = path.join(ROOT, 'target', release ? 'release' : 'debug', 'SHIKISHA-TERM.exe');
  if (!fs.existsSync(exe)) throw new Error(`no build at ${exe} -- run cargo build${release ? ' --release' : ''} first`);
  stop();
  await sleep(800);
  fs.rmSync(run, { recursive: true, force: true });
  for (const d of [app, local]) fs.mkdirSync(d, { recursive: true });
  const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', app, '-Package', '-Exe', exe);
  if (!fs.existsSync(path.join(app, 'SHIKISHA-TERM.exe'))) throw new Error('staging failed:\n' + staged.stdout + staged.stderr);
  fs.mkdirSync(path.join(app, 'config'), { recursive: true });
  fs.writeFileSync(path.join(app, 'config', 'config.json'), JSON.stringify(config, null, 2));
  const env = cleanEnv();
  env.LOCALAPPDATA = local;
  if (args.length) env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = args.join(' ');
  spawn(path.join(app, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: app, env, detached: true, stdio: 'ignore' }).unref();
  // CPU seconds spent so far by the app and every browser process it started
  const cpuSeconds = () => Number(ps('-Command',
    `$t = 0; Get-CimInstance Win32_Process | Where-Object { ` +
    `($_.ExecutablePath -and $_.ExecutablePath -like '${run}\\*') -or ` +
    `(($_.Name -eq 'msedgewebview2.exe' -or $_.Name -eq 'chrome.exe') -and ($_.CommandLine + '') -like '*${path.basename(run)}\\localappdata*') } | ` +
    `ForEach-Object { $t += [double]$_.KernelModeTime + [double]$_.UserModeTime }; $t / 1e7`).stdout.trim()) || 0;
  return { home: app, data: path.join(app, 'data'), logs: path.join(app, 'logs'), stop, cpuSeconds };
}

function startMac({ run, config, args }) {
  const app = macApp();
  if (!fs.existsSync(path.join(app, 'Contents', 'MacOS', 'SHIKISHA-TERM'))) {
    throw new Error(`no app at ${app} -- make one with packaging/mac/mkapp.sh (see tools/debug/lib/copy.mjs), or name it in SHIKISHA_APP`);
  }
  const home = path.join(run, 'home');
  const work = path.join(run, 'work');
  // This copy's processes and no other's: the app's own, and Chromium's
  // helpers inside it, holding this copy's folder in their environment
  const mine = () => {
    const listed = spawnSync('ps', ['-AEww', '-o', 'pid=,time=,command='], { encoding: 'utf8' }).stdout || '';
    return listed.split('\n').map((l) => l.trim()).filter((l) => l.includes(app + '/Contents/') && l.includes(`SHIKISHA_HOME=${home}`))
      .map((l) => { const [pid, time, ...rest] = l.split(/\s+/); return { pid: Number(pid), time, command: rest.join(' ') }; });
  };
  // Asked to end the way the system asks, and only the program itself: it
  // takes its helpers down with it. Ended together with its helpers, it sees
  // its browser vanish under it and says so in a dialog nobody is there for
  const stop = () => {
    const main = path.join(app, 'Contents', 'MacOS', 'SHIKISHA-TERM');
    for (const p of mine()) if (p.command.startsWith(main + ' ') || p.command === main) { try { process.kill(p.pid, 'SIGTERM'); } catch {} }
    const until = Date.now() + 8000;
    while (mine().length && Date.now() < until) spawnSync('sleep', ['0.2']);
    for (const p of mine()) { try { process.kill(p.pid, 'SIGKILL'); } catch {} }
  };
  stop();
  fs.rmSync(run, { recursive: true, force: true });
  for (const d of [path.join(home, 'config'), work]) fs.mkdirSync(d, { recursive: true });
  fs.writeFileSync(path.join(home, 'config', 'config.json'), JSON.stringify(config, null, 2));
  const chromium = ['--use-mock-keychain', ...args].join(' ');
  const opened = spawnSync('open', ['-n', '-g', '--env', `SHIKISHA_HOME=${home}`, '--env', `SHIKISHA_CHROMIUM_ARGS=${chromium}`, app],
    { encoding: 'utf8', env: cleanEnv() });
  if (opened.status !== 0) throw new Error('the app would not open: ' + opened.stderr);
  // "[[dd-]hh:]mm:ss.cc", user and system together
  const seconds = (t) => t.split(/[-:]/).map(Number).reverse().reduce((s, v, i) => s + v * [1, 60, 3600, 86400][i], 0);
  const cpuSeconds = () => mine().reduce((s, p) => s + seconds(p.time), 0);
  return { home, data: path.join(home, 'data'), logs: path.join(home, 'logs'), work, stop, cpuSeconds };
}

/** Waits for the copy's relay to say its token, and gives it */
export async function tokenOf(copy, ms = 40000) {
  const file = path.join(copy.data, 'remote-token');
  for (const end = Date.now() + ms; Date.now() < end; await sleep(250)) {
    try { const t = fs.readFileSync(file, 'utf8').trim(); if (t) return t; } catch {}
  }
  throw new Error('the relay never opened -- see ' + copy.logs);
}

export const shellCommand = () => (process.platform === 'win32' ? 'cmd.exe' : '/bin/sh');
export const tmp = (name) => path.join(os.tmpdir(), name);
