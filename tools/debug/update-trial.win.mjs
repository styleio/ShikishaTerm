/**
 * A version an update put in place that stops twice while starting offers
 * the way back, and takes it.
 *
 * This checkout's build in a folder of its own, set up the way an update
 * leaves it: the version it replaced kept under data\update\prev (a real
 * published one, fetched from the releases), and the new version on trial
 * with two failed starts already counted. Started, it must ask; answered:
 *   --answer=yes (the default): the kept version is put back and started,
 *     the failed one is passed over and the trial is over
 *   --answer=no: this version goes on starting, on trial afresh
 *
 *     cargo build
 *     node tools/debug/update-trial.win.mjs [--answer=yes|no] [--back=0.26.0] [--lang=ja]
 *
 * Needs Windows, Node and the network (once, for the published zip). The
 * question is a system message box on this screen; it is answered by its
 * own button, found by its id, never by a key pressed into whatever has
 * the focus. Nothing of a copy somebody is using is read, written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const arg = (name, dflt) => (process.argv.find((a) => a.startsWith(`--${name}=`)) || `--${name}=${dflt}`).split('=')[1];
const ANSWER = arg('answer', 'yes');
const BACK = arg('back', '0.26.0');
const LANG = arg('lang', 'en');
const RUN = path.join(os.tmpdir(), 'sk-update-trial');
const APP = path.join(RUN, 'app');
const CACHE = path.join(os.tmpdir(), 'sk-update-trial-cache', BACK);
const NOW = /^version\s*=\s*"([^"]+)"/m.exec(fs.readFileSync(path.join(ROOT, 'Cargo.toml'), 'utf8'))[1];

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and ($_.CommandLine + '') -like '*sk-update-trial*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);
const hash = (f) => crypto.createHash('sha256').update(fs.readFileSync(f)).digest('hex');
const read = (f) => (fs.existsSync(f) ? fs.readFileSync(f, 'utf8') : '');

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');
if (!['yes', 'no'].includes(ANSWER)) die('--answer is yes or no');

// The version to go back to: the published zip, unpacked once and kept
const held = ['SHIKISHA-TERM.exe', 'conpty.dll', 'OpenConsole.exe'];
if (!fs.existsSync(path.join(CACHE, 'SHIKISHA-TERM.exe'))) {
  fs.mkdirSync(CACHE, { recursive: true });
  const zip = path.join(CACHE, 'SHIKISHA-TERM.zip');
  const url = `https://github.com/styleio/ShikishaTerm/releases/download/v${BACK}/SHIKISHA-TERM.zip`;
  console.log('fetching ' + url);
  const r = await fetch(url);
  if (!r.ok) die(`could not fetch ${url}: ${r.status}`);
  fs.writeFileSync(zip, Buffer.from(await r.arrayBuffer()));
  const out = path.join(CACHE, 'unpacked');
  const un = ps('-Command', `Expand-Archive -LiteralPath '${zip}' -DestinationPath '${out}' -Force`);
  if (un.status !== 0) die('could not unpack: ' + un.stderr);
  for (const name of held) {
    const found = ps('-Command', `(Get-ChildItem -LiteralPath '${out}' -Recurse -Filter '${name}' | Select-Object -First 1).FullName`).stdout.trim();
    if (found) fs.copyFileSync(found, path.join(CACHE, name));
  }
}
const backHash = hash(path.join(CACHE, 'SHIKISHA-TERM.exe'));

stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
fs.mkdirSync(path.join(APP, 'config'), { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) die('staging failed:\n' + staged.stdout + staged.stderr);
const nowHash = hash(appExe);
const answered = {};
for (const f of fs.readdirSync(path.join(ROOT, 'profiles')).filter((f) => f.endsWith('.json'))) {
  const p = JSON.parse(fs.readFileSync(path.join(ROOT, 'profiles', f), 'utf8'));
  if (p.resume && p.resume.hook) answered[p.name] = 'off';
}
fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
  language: LANG, agent_hooks: answered, update: { auto: false },
  desks: [{ name: 'Trial', id: 'trial', folders: [{ cwd: RUN, tabs: [{ name: 'shell', id: 'shell', command: ['cmd.exe'] }] }] }],
}, null, 2));

// What an update leaves: the version it replaced kept, and the new one on
// trial -- here with two failed starts counted already
const update = path.join(APP, 'data', 'update');
fs.mkdirSync(path.join(update, 'prev', BACK), { recursive: true });
for (const name of held) {
  if (fs.existsSync(path.join(CACHE, name))) fs.copyFileSync(path.join(CACHE, name), path.join(update, 'prev', BACK, name));
}
fs.writeFileSync(path.join(update, 'trial.json'), JSON.stringify({ version: NOW, from: BACK, starts: 2 }));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = path.join(RUN, 'localappdata');
const child = spawn(appExe, ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' });
const pid = child.pid;
child.unref();

// The message box of that process: its words (the static text, id 65535),
// and one of its buttons (Yes is 6, No is 7) clicked by its own window
const box = (press) => ps('-Command', `
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type -Namespace W -Name U -MemberDefinition '[DllImport("user32.dll")] public static extern System.IntPtr SendMessageW(System.IntPtr h, uint m, System.IntPtr w, System.IntPtr l);'
$root = [System.Windows.Automation.AutomationElement]::RootElement
$cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ProcessIdProperty, ${pid})
foreach ($w in $root.FindAll([System.Windows.Automation.TreeScope]::Children, $cond)) {
  if ($w.Current.ClassName -ne '#32770') { continue }
  $byId = { param($id) $w.FindFirst([System.Windows.Automation.TreeScope]::Descendants, (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::AutomationIdProperty, $id))) }
  $text = (& $byId '65535').Current.Name -replace '\\s+', ' '
  Write-Output "TEXT:$text"
  if ('${press}') {
    $b = & $byId '${press}'
    [void][W.U]::SendMessageW([System.IntPtr]$b.Current.NativeWindowHandle, 0x00F5, [System.IntPtr]::Zero, [System.IntPtr]::Zero)
    Write-Output 'PRESSED'
  }
}`).stdout;

const until = async (test, what, ms) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { const v = await test(); if (v) return v; await sleep(700); }
  throw new Error('timed out waiting for ' + what);
};

const log = () => read(path.join(APP, 'logs', 'hooks.log'));
try {
  const said = await until(async () => /TEXT:(.+)/.exec(box(''))?.[1], 'the question', 30000);
  console.log('   asked: ' + said);
  check(said.includes(NOW) && said.includes(BACK), 'the question names the version that failed and the one to go back to');
  check(box(ANSWER === 'yes' ? '6' : '7').includes('PRESSED'), `"${ANSWER}" is pressed`);

  if (ANSWER === 'yes') {
    await until(async () => log().includes(`Update: now running ${BACK} (was ${NOW})`), `${BACK} to be running`, 60000);
    check(true, `${BACK} is put back and started`);
    check(hash(appExe) === backHash && hash(appExe) !== nowHash, 'the program in the folder is the kept version');
    check(read(path.join(APP, 'data', 'update-skipped')).trim() === NOW, `${NOW} is passed over, not offered again`);
    check(!fs.existsSync(path.join(update, 'trial.json')), 'the trial is over');
  } else {
    await until(async () => log().includes('Sending state'), 'this version to start', 60000);
    check(hash(appExe) === nowHash, 'the program in the folder is still this version');
    const trial = JSON.parse(read(path.join(update, 'trial.json')) || '{}');
    check(trial.version === NOW && trial.starts === 1, `this version is on trial afresh (${JSON.stringify(trial)})`);
    check(!read(path.join(APP, 'data', 'update-skipped')).includes(NOW), 'this version is not passed over');
  }
} catch (e) {
  console.error(e);
  failures += 1;
} finally {
  stopApp();
}
console.log(failures ? `${failures} check(s) failed` : 'all checks passed');
process.exit(failures ? 1 : 0);
