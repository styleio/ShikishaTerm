/**
 * A page's notifications, through the running app's own window on Windows.
 *
 * This checkout's build in a folder of its own, with a small site served here
 * in a browser tab. The page asks to show notifications the way any site does;
 * the board puts the question, is answered the way a person answers it, and
 * what the page is told and what it shows afterwards is read back.
 *
 *     cargo build
 *     node tools/debug/page-notice.win.mjs [--ja]
 *
 * Checked:
 *   shim      the page's Notification is this program's: "default" before
 *             anybody answers, "prompt" to navigator.permissions
 *   ask       requestPermission puts one question on the board, naming the
 *             site, and waits for it
 *   allow     "Allow" settles the page's promise with "granted", and a
 *             notification the page then makes reaches the banner (nothing
 *             dropped or refused in the log) and Windows keeps the banner,
 *             the site's name in front (skipped where this PC has
 *             notifications turned off)
 *   kept      the page loaded again knows the answer without a question
 *   block     another site blocked: "denied", what it makes fails on the page,
 *             and the same said straight to the program is dropped
 *
 * Needs Windows and Node. Photographs land in target/shots.
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import http from 'node:http';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const SHOTS = path.join(ROOT, 'target', 'shots');
const RUN = path.join(os.tmpdir(), 'sk-page-notice');
const APP = path.join(RUN, 'app');
const WORK = path.join(RUN, 'work');
const LOCAL = path.join(RUN, 'localappdata');
const CONFIG = path.join(APP, 'config', 'config.json');
const JA = process.argv.includes('--ja');
const L = JSON.parse(fs.readFileSync(path.join(ROOT, 'lang', (JA ? 'ja' : 'en') + '.json'), 'utf8'));

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-Process -Name 'SHIKISHA-TERM' -ErrorAction SilentlyContinue | ` +
  `Where-Object { $_.Path -and $_.Path -like '${RUN}\\*' } | ` +
  `ForEach-Object { & taskkill.exe /PID $_.Id /T /F 2>&1 | Out-Null }`);
const until = async (test, what, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(200); }
  throw new Error('timed out waiting for ' + what);
};
const log = () => { try { return fs.readFileSync(path.join(APP, 'logs', 'hooks.log'), 'utf8'); } catch { return ''; } };

const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

// Two sites: one to allow, one to block. Two ports of this machine are two
// sites, the way a browser counts them
const PAGE = '<!doctype html><html><head><meta charset="utf-8"><title>News</title></head><body><p>news</p></body></html>';
const serve = async () => {
  const s = http.createServer((req, res) => {
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
    res.end(PAGE);
  });
  await new Promise((r) => s.listen(0, '127.0.0.1', r));
  return s;
};
const servers = [await serve(), await serve()];
const [siteA, siteB] = servers.map((s) => `http://127.0.0.1:${s.address().port}/`);

// The first banner a copy shows writes the person's Start Menu shortcut to
// point at that copy (Windows accepts banners only from a program a shortcut
// names). Kept as it was and put back at the end, so the installed copy's
// shortcut does not end up naming this check's folder after it is gone
const LINK = path.join(process.env.APPDATA, 'Microsoft', 'Windows', 'Start Menu', 'Programs', 'SHIKISHA-TERM.lnk');
const linkWas = fs.existsSync(LINK) ? fs.readFileSync(LINK) : null;
const putLinkBack = () => {
  if (linkWas) fs.writeFileSync(LINK, linkWas);
  else fs.rmSync(LINK, { force: true });
};

console.log('starting this checkout\'s build, isolated');
stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, WORK, LOCAL, SHOTS]) fs.mkdirSync(d, { recursive: true });
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
fs.mkdirSync(path.dirname(CONFIG), { recursive: true });
fs.writeFileSync(CONFIG, JSON.stringify({
  language: JA ? 'ja' : 'en',
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
  desks: [{ name: 'News', id: 'news', folders: [{ cwd: WORK, tabs: [
    { name: 'a', id: 'a', command: `browser ${siteA}` },
    { name: 'b', id: 'b', command: `browser ${siteB}` },
  ] }] }],
}, null, 2));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();

const portOf = (envName) => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', envName, 'EBWebView', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return null;
  const n = Number(fs.readFileSync(f, 'utf8').split(/\r?\n/)[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};
const targetsOf = async (port) => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json());
const pageAt = async (site) => {
  let t;
  await until(async () => {
    const p = portOf(path.join('profiles', 'default'));
    return p && (t = (await targetsOf(p)).find((x) => x.type === 'page' && x.url.startsWith(site)));
  }, 'the page at ' + site, 30000);
  return connectCdp(t);
};
// What Windows keeps of this program's banners: the words of each, as the
// notification centre has them (Windows PowerShell: it reaches WinRT)
const banners = () => {
  const r = spawnSync('powershell.exe', ['-NoProfile', '-Command',
    "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; " +
    "[Windows.UI.Notifications.ToastNotificationManager]::History.GetHistory('WIREDECOK.K.SHIKISHA-TERM.Portable') | " +
    "ForEach-Object { $_.Content.GetXml() }"], { encoding: 'utf8' });
  return r.stdout.split(/\r?\n/).filter((l) => l.trim());
};
// Notifications turned off for the whole of this PC: nothing is shown or kept
const toastsOff = () => {
  const r = spawnSync('reg', ['query', 'HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\PushNotifications', '/v', 'ToastEnabled'], { encoding: 'utf8' });
  return /ToastEnabled\s+REG_DWORD\s+0x0\b/.test(r.stdout);
};
try {
  let boardTarget;
  await until(async () => {
    const p = portOf('shell');
    return p && (boardTarget = (await targetsOf(p)).find((t) => t.type === 'page'));
  }, 'the window\'s page', 40000);
  const board = await connectCdp(boardTarget);
  await until(() => board.run('!!S && S.tabs.filter(t => t.kind === "browser").length === 2'), 'the board and both pages', 30000);
  const tabOf = (id) => board.run(`S.tabs.find(t => (t.id || t.name) === ${JSON.stringify(id)}).index`);
  const front = async (id) => {
    const tab = await tabOf(id);
    await board.run(`send({kind:"select", tab:${tab}})`);
    await until(() => board.run(`S.active === ${tab}`), 'page ' + id + ' in front');
  };
  await front('a');
  const a = await pageAt(siteA);
  await until(() => a.run('document.readyState === "complete"'), 'page a loaded');

  console.log('1. the page\'s Notification is this program\'s');
  check(await a.run('Notification.permission') === 'default', 'nobody has answered: "default"');
  check(await a.run('navigator.permissions.query({name:"notifications"}).then(s => s.state)') === 'prompt', 'and the permissions API says "prompt"');

  console.log('2. the page asks');
  await a.run('window.__asked = "waiting"; Notification.requestPermission().then(r => window.__asked = r); true');
  await until(() => board.run('(S.notice_asks || []).length === 1'), 'the question on the board', 10000)
    .catch((e) => { throw new Error(e.message + ': ' + log().split('\n').slice(-8).join('\n')); });
  const asked = JSON.parse(await board.run('JSON.stringify(S.notice_asks[0])'));
  check(siteA.startsWith(asked.site) && asked.host.startsWith('127.0.0.1:'), 'one question, naming the site: ' + JSON.stringify(asked));
  check(await board.run(`document.body.innerText.includes(${JSON.stringify(L['tui.notice.allow'])})`), 'the board shows it with an Allow');
  check(await a.run('window.__asked') === 'waiting', 'the page waits for the answer');
  await sleep(300);
  fs.writeFileSync(path.join(SHOTS, `page-notice-${JA ? 'ja-' : ''}ask.png`),
    Buffer.from((await board.send('Page.captureScreenshot', { format: 'png' })).data, 'base64'));

  console.log('3. allowed');
  await board.run(`send({kind:"notice-answer", site:${JSON.stringify(asked.site)}, allow:true})`);
  await until(() => a.run('window.__asked === "granted"'), 'the page told', 10000).catch(() => {});
  check(await a.run('window.__asked') === 'granted', 'the promise settles "granted": ' + await a.run('window.__asked'));
  check(await a.run('Notification.permission') === 'granted', 'Notification.permission is "granted"');
  check(await board.run('(S.notice_asks || []).length === 0'), 'the question is gone');
  const before = log().length;
  await a.run('new Notification("Build finished", {body: "All 12 checks passed"}); true');
  await sleep(2500);
  const said = log().slice(before);
  check(!/notice: (dropped|not shown)/.test(said), 'the notification is shown, nothing dropped or refused' + (said.trim() ? ': ' + said.trim() : ''));
  if (toastsOff()) {
    console.log('  SKIP Windows has the banner: notifications are turned off for the whole of this PC (Settings > System > Notifications), so no program\'s banner is kept');
  } else {
    const kept = banners();
    check(kept.some((x) => x.includes('Build finished') && x.includes('All 12 checks passed') && x.includes('127.0.0.1:')),
      'Windows has the banner, the site\'s name in front of the page\'s words: ' + JSON.stringify(kept));
  }

  console.log('4. kept');
  await a.send('Page.reload', {});
  await sleep(1500);
  await until(() => a.run('document.readyState === "complete"'), 'page a again');
  await until(() => a.run('Notification.permission === "granted"'), 'the answer known', 8000).catch(() => {});
  check(await a.run('Notification.permission') === 'granted', 'loaded again, the page knows "granted" without asking');
  check(await board.run('(S.notice_asks || []).length === 0'), 'and no question is put');

  console.log('5. another site, blocked');
  await front('b');
  const b = await pageAt(siteB);
  await until(() => b.run('document.readyState === "complete"'), 'page b loaded');
  await b.run('window.__asked = "waiting"; Notification.requestPermission().then(r => window.__asked = r); true');
  await until(() => board.run('(S.notice_asks || []).length === 1'), 'the question for b', 10000);
  const askedB = JSON.parse(await board.run('JSON.stringify(S.notice_asks[0])'));
  check(siteB.startsWith(askedB.site), 'the question names the other site');
  await board.run(`send({kind:"notice-answer", site:${JSON.stringify(askedB.site)}, allow:false})`);
  await until(() => b.run('window.__asked === "denied"'), 'b told', 10000).catch(() => {});
  check(await b.run('window.__asked') === 'denied', 'blocked: the promise settles "denied"');
  await b.run('window.__shown = "nothing"; const n = new Notification("Buy now"); n.onerror = () => window.__shown = "error"; n.onshow = () => window.__shown = "show"; true');
  await until(() => b.run('window.__shown !== "nothing"'), 'what came of it', 5000).catch(() => {});
  check(await b.run('window.__shown') === 'error', 'what it makes fails on the page, as a browser\'s does: ' + await b.run('window.__shown'));
  // A page that goes round its own Notification and says it straight to the
  // program is still not shown: the answer is the program's to keep
  const beforeB = log().length;
  await b.run('window.__shikisha_post(JSON.stringify({kind: "notice", title: "Buy now", body: "really"})); true');
  await sleep(1500);
  check(/notice: dropped/.test(log().slice(beforeB)), 'said straight to the program, it is dropped, not shown');
  check(await a.run('Notification.permission') === 'granted', 'the first site is still allowed');
} catch (e) {
  console.error(e.stack || e.message);
  failures += 1;
} finally {
  stopApp();
  putLinkBack();
  for (const s of servers) s.close();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
