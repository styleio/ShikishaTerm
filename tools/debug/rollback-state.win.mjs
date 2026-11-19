/**
 * Going back to an older version from the settings, after this version took
 * this PC's records across: the records are handed back to the `data` folder
 * as the run ends -- with what the run wrote last in them, and without a
 * stale database journal left there from before -- and the older version
 * starts on them.
 *
 * This checkout's build in a folder of its own, with a real published
 * version kept the way an update keeps the one it replaced. Started once (it
 * takes the records across), then asked to go back by the settings' own
 * request, as the button asks it.
 *
 *     cargo build
 *     node tools/debug/rollback-state.win.mjs [--back=0.26.0]
 *
 * Needs Windows, Node and the network (once, for the published zip, shared
 * with update-trial.win.mjs). Nothing of a copy somebody is using is read,
 * written or stopped.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { stateFolder } from './lib/copy.mjs';

const arg = (name, def) => (process.argv.find((a) => a.startsWith(`--${name}=`)) || '').split('=')[1] || def;
const BACK = arg('back', '0.26.0');
const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-rollback-state');
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'localappdata');
const STATE = stateFolder(APP, LOCAL);
const DATA = path.join(APP, 'data');
const CACHE = path.join(os.tmpdir(), 'sk-update-trial-cache', BACK);
const PORT = 9487;
const TOKEN = 'rollback-token-0123456789';
const BASE = `http://127.0.0.1:${PORT}`;
const held = ['SHIKISHA-TERM.exe', 'conpty.dll', 'OpenConsole.exe'];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const ps = (...a) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...a], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process | Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }; ` +
  `Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and ($_.CommandLine + '') -like '*sk-rollback-state*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);
const hash = (f) => crypto.createHash('sha256').update(fs.readFileSync(f)).digest('hex');
const read = (f) => { try { return fs.readFileSync(f, 'utf8'); } catch { return ''; } };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const until = async (test, what, ms = 60000) => {
  for (const end = Date.now() + ms; Date.now() < end; await sleep(500)) { try { if (await test()) return; } catch {} }
  throw new Error('timed out waiting for ' + what);
};
const log = () => read(path.join(APP, 'logs', 'hooks.log'));

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) throw new Error('no build at target\\debug -- run cargo build first');
if (!fs.existsSync(path.join(CACHE, 'SHIKISHA-TERM.exe'))) {
  throw new Error(`no ${BACK} kept at ${CACHE} -- run tools/debug/update-trial.win.mjs --back=${BACK} once, which fetches it`);
}
const backHash = hash(path.join(CACHE, 'SHIKISHA-TERM.exe'));

stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
fs.mkdirSync(path.join(APP, 'config'), { recursive: true });
fs.mkdirSync(LOCAL, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
const appExe = path.join(APP, 'SHIKISHA-TERM.exe');
if (!fs.existsSync(appExe)) throw new Error('staging failed:\n' + staged.stdout + staged.stderr);
fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
  language: 'en', update: { auto: false },
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  remote: { enabled: true, bind: '127.0.0.1', port: PORT, sticky_token: true, fixed_token: TOKEN },
  desks: [{ name: 'Back', id: 'back', folders: [{ cwd: RUN, tabs: [{ name: 'shell', id: 'shell', command: ['cmd.exe'] }] }] }],
}, null, 2));
// The version this one replaced, kept the way an update keeps it
const prev = path.join(DATA, 'update', 'prev', BACK);
fs.mkdirSync(prev, { recursive: true });
for (const name of held) if (fs.existsSync(path.join(CACHE, name))) fs.copyFileSync(path.join(CACHE, name), path.join(prev, name));
// What the older version left in `data`: a record, and the journal of a
// database the newer run will no longer have beside it
fs.writeFileSync(path.join(DATA, 'remote-token'), 'left-by-the-older-version-0123');
fs.writeFileSync(path.join(DATA, 'conversations.db-wal'), 'a journal from before');

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
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

const jar = new Map();
const send = async (url, init = {}) => {
  const r = await fetch(BASE + url, { redirect: 'manual', ...init,
    headers: { ...(init.headers || {}), Cookie: [...jar].map(([k, v]) => `${k}=${v}`).join('; ') },
    signal: AbortSignal.timeout(15000) });
  for (const kv of r.headers.getSetCookie().map((c) => c.split(';')[0])) { const i = kv.indexOf('='); jar.set(kv.slice(0, i), kv.slice(i + 1)); }
  return r;
};

try {
  console.log(`1. this version starts and takes the records across`);
  await until(() => fs.existsSync(path.join(STATE, 'program-folder.txt')), 'the records to be taken across');
  await until(async () => (await send(`/?t=${TOKEN}`)).status < 500 && jar.has('rs'), 'the board');
  await until(() => fs.existsSync(path.join(STATE, 'conversations.db')), 'the conversation record to be opened');
  // The settings are reached the way a phone reaches them: the link's key
  // traded for a cookie, and the relay adding the settings' own key itself
  await send(`/cfg?t=${TOKEN}`);
  check(jar.has('sst'), 'the settings let this in');

  console.log(`2. going back to ${BACK} from the settings`);
  const askedAt = Date.now() - 2000;
  const asked = await send('/api/update/rollback', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
  check(asked.status === 200, `the request was taken (${asked.status} ${await asked.text().catch(() => '')})`);
  // The quit question a running tab brings, answered Yes by its own button
  await until(() => box('6').includes('PRESSED') || /State: handed back to/.test(log()), 'the quit question', 30000);
  await until(() => /State: handed back to/.test(log()), 'the records to be handed back', 90000);
  await until(() => hash(appExe) === backHash, `${BACK} to be in the folder`, 90000);
  check(true, `${BACK} is put back`);
  check(!fs.existsSync(path.join(STATE, 'program-folder.txt')), 'the state folder is no longer taken for the copy\'s');
  check(fs.existsSync(path.join(DATA, 'conversations.db')), 'the conversation record is in the data folder');
  check(!fs.existsSync(path.join(DATA, 'conversations.db-wal')) || read(path.join(DATA, 'conversations.db-wal')) !== 'a journal from before',
    'the stale journal is gone from beside it');
  // The run's last write -- the tabs as they were on the way out -- is in
  // what was handed back (a copy keeps the time a file was written)
  const session = path.join(DATA, 'last-session');
  check(fs.existsSync(session) && fs.statSync(session).mtimeMs >= askedAt, 'what the run wrote on its way out was handed back too');
  check(log().includes(`-> ${BACK} put in place`), 'the way back was put in place');
  // The older version started, and is still running a while later: it did
  // not give up waiting for this one and leave with nothing running
  await until(() => log().includes(`Update: now running ${BACK}`), `${BACK} to say it runs`, 60000);
  await sleep(10000);
  const running = ps('-Command', `@(Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | Where-Object { $_.Path -like '${APP}\\*' }).Count`).stdout.trim();
  check(Number(running) >= 1, `${BACK} is still running ten seconds later (${running} running)`);
} finally {
  stopApp();
}
console.log(failures ? `${failures} FAILED` : 'all checks passed');
process.exit(failures ? 1 : 0);
