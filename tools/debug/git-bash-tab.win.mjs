// Check that a tab can be made a Git Bash from the Command kind, on a PC that
// has Git for Windows, and that a command under a path with spaces is kept
// whole: shown as one line with its program quoted, written as its words
// apart, started, and left as it was by a save that did not touch it.
// Run after cargo build: node tools/debug/git-bash-tab.win.mjs
// Windows + Node + Git for Windows. Uses instance.win.ps1, starts only
// disposable shells, and never touches installed settings. Pictures go to
// target/shots/git-bash-tab.
import fs from 'node:fs';
import path from 'node:path';
import {spawnSync} from 'node:child_process';
import assert from 'node:assert/strict';
import {connectCdp} from './chrome.mjs';

const root = path.resolve(import.meta.dirname, '../..');
const bashes = ['ProgramFiles', 'ProgramW6432'].map(k => process.env[k]).filter(Boolean)
  .map(p => path.join(p, 'Git', 'bin', 'bash.exe')).filter(p => fs.existsSync(p));
if (!bashes.length) { console.log('SKIP no Git for Windows under Program Files'); process.exit(0); }
const area = fs.mkdtempSync(path.join(root, 'target', 'git-bash-tab-'));
const instance = path.join(area, 'instance');
const config = path.join(area, 'settings.json');
const work = path.join(area, 'work'); fs.mkdirSync(work);
// A tab written the way a project added with Git Bash as its default command
// is written (config::command_value): saved from the settings screen, it has
// to come back as it went in
const written = [bashes[0], '--login', '-i'];
fs.writeFileSync(config, JSON.stringify({language:'ja', keep_terminals:false,
  agent_hooks:{'Claude Code':'off','Codex CLI':'off','Gemini CLI':'off'},
  desks:[{name:'Bash check', id:'bash-check', folders:[
    {name:'Here', cwd:work, tabs:[{name:'Old bash', id:'old-bash', command:written}]},
  ]}]}));
const ps = args => {
  const r = spawnSync('powershell.exe', ['-NoProfile','-ExecutionPolicy','Bypass','-File',
    path.join(root,'tools/debug/instance.win.ps1'),'-At',instance,...args],
    {cwd:root,encoding:'utf8',windowsHide:true,timeout:90000,
     env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=0'}});
  if (r.status !== 0) throw Error(r.stderr || r.stdout);
  return r.stdout;
};
const sleep = ms => new Promise(r => setTimeout(r,ms));
const check = (ok, why) => {assert(ok,why);console.log('PASS '+why);};
const safeError = e => String(e?.stack || e).replace(/token=[^&\s)]+/g,'token=hidden');
async function until(test, why) {
  for (let n=0;n<100;n++) { try { const v=await test(); if(v) return v; } catch {} await sleep(200); }
  throw Error('Timed out: '+why);
}
const sockets=[];
async function attach(target) {
  const {ws, send:call, run:js} = await connectCdp(target, {timeout:15000});
  sockets.push(ws);
  await call('Page.enable');
  return {call,js};
}
try {
  const app = Number(/pid=(\d+)/.exec(ps(['-Config',config,'-Work',work,'-Cdp','0']))[1]);
  // The Git Bash shells this app started: every bash.exe below it run as a
  // login shell -- one per tab that started, none for one that did not
  const shells = () => {
    const r = spawnSync('powershell.exe', ['-NoProfile', '-Command',
      'Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name,CommandLine | ConvertTo-Json -Compress'],
      {encoding:'utf8', windowsHide:true, timeout:60000});
    const all = JSON.parse(r.stdout);
    const parent = new Map(all.map(p => [p.ProcessId, p.ParentProcessId]));
    const below = pid => { for (let p = parent.get(pid), n = 0; p && n < 20; p = parent.get(p), n++) if (p === app) return true; return false; };
    return all.filter(p => /^bash\.exe$/i.test(p.Name) && /--login/.test(p.CommandLine || '') && /Git\\bin\\bash\.exe/i.test(p.CommandLine || '') && below(p.ProcessId)).length;
  };
  const targets=async(name)=>{
    const file=path.join(instance,'localappdata/ShikishaTerm/webview2',name,'EBWebView/DevToolsActivePort');
    const port=Number(fs.readFileSync(file,'utf8').split(/\r?\n/)[0]);
    return (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).filter(t=>t.type==='page');
  };
  const board=await attach(await until(async()=>(await targets('shell'))[0],'board'));
  await until(()=>board.js('S?.tabs?.some(t=>t.id === "old-bash") && !!document.querySelector("#strip .snew")'),'tab plus');
  const savedFile=path.join(instance,'app/config/config.json');
  const saved=()=>JSON.parse(fs.readFileSync(savedFile,'utf8'));
  await board.js('document.querySelector("#strip .snew").click()');
  const settings=await attach(await until(async()=>(await targets('profiles/default')).find(t=>t.url.includes('addtab=')),'settings child'));
  await until(()=>settings.js('framed?.kind === "addtab"'),'add tab dialog');

  // The kind: Command, which names Git Bash on a PC that has it
  const kind = await settings.js(`(() => { const s = [...document.querySelectorAll("#floatbody select")]
    .find(s => [...s.options].some(o => o.value === "cmd")); s.value = "cmd";
    s.dispatchEvent(new Event("change")); return s.selectedOptions[0].textContent; })()`);
  check(kind.includes('Git Bash'), 'the Command kind says Git Bash is among its shells: ' + kind);
  // The shell: Git Bash, beside PowerShell and the command prompt
  const offered = await until(()=>settings.js(`(() => { const s = [...document.querySelectorAll("#floatbody select")]
    .find(s => [...s.options].some(o => o.textContent === "Git Bash")); return s && [...s.options].map(o => o.textContent).join(" / "); })()`), 'shell list');
  check(/PowerShell/.test(offered) && /Git Bash/.test(offered), 'the shell list offers Git Bash: ' + offered);
  await settings.js(`(() => { const s = [...document.querySelectorAll("#floatbody select")]
    .find(s => [...s.options].some(o => o.textContent === "Git Bash"));
    s.value = [...s.options].find(o => o.textContent === "Git Bash").value; s.dispatchEvent(new Event("change")); })()`);
  const field = await settings.js('document.querySelector("#floatbody input.mono").value');
  check(/^"[^"]+\\bash\.exe" --login -i$/.test(field), 'the command field shows one line, its program quoted: ' + field);
  const cmd = await settings.js('JSON.stringify(framed.t.command)');
  check(Array.isArray(JSON.parse(cmd)) && JSON.parse(cmd)[0].endsWith('bash.exe') && JSON.parse(cmd)[0].includes(' '), 'and it is held as its words apart: ' + cmd);
  check(await settings.js('framed.t.name') === 'Git Bash', 'the tab is named Git Bash');
  // The line the app would really run, asked of the app
  const real = await until(()=>settings.js(`(() => { const c = document.querySelector("#floatbody .realcmd code"); return c && c.textContent.includes("bash.exe") && c.textContent; })()`), 'real command line');
  check(/^"[^"]*Program Files[^"]*bash\.exe" --login -i$/.test(real), 'the app reads it as one program with its arguments, shown quoted: ' + real);
  const shots=path.join(root,'target/shots/git-bash-tab'); fs.mkdirSync(shots,{recursive:true});
  fs.writeFileSync(path.join(shots,'add.png'),Buffer.from((await settings.call('Page.captureScreenshot',{format:'png'})).data,'base64'));

  await settings.js('document.getElementById("floatadd").click()');
  await until(()=>board.js('!S.settings_open && S.tabs.filter(t=>t.kind === "pty").length === 2'),'added tab arrives on the board');
  const tabs = saved().desks[0].folders[0].tabs;
  check(JSON.stringify(tabs[0].command) === JSON.stringify(written), 'the tab written apart is saved as it was: ' + JSON.stringify(tabs[0].command));
  check(Array.isArray(tabs[1].command) && tabs[1].command[0].endsWith('bash.exe'), 'the new tab is saved as its words apart: ' + JSON.stringify(tabs[1].command));
  // Started: a Git Bash runs under the app for each tab, the one written
  // before and the one just made
  const running = await until(() => shells() >= 2 && shells(), 'two Git Bash shells under the app');
  check(running >= 2, 'both Git Bash tabs started: ' + running + ' login shells under the app');
  console.log('All git-bash-tab checks passed. Fixture: '+area);
} catch(e) {
  console.error(safeError(e)); process.exitCode=1;
} finally { for(const ws of sockets) ws.close(); ps(['-Stop']); }
