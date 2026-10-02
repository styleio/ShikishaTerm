// Check the tab bar's +, cancellation, More settings and saving.
// Run after cargo build: node tools/debug/add-tab.win.mjs
// Windows + Node. Uses instance.win.ps1, starts only a disposable cmd.exe, and
// never touches installed settings. Pictures go to target/shots/add-tab.
import fs from 'node:fs';
import path from 'node:path';
import {spawnSync} from 'node:child_process';
import assert from 'node:assert/strict';

const root = path.resolve(import.meta.dirname, '../..');
const area = fs.mkdtempSync(path.join(root, 'target', 'add-tab-check-'));
const instance = path.join(area, 'instance');
assert.equal(path.relative(area, instance), 'instance');
assert(!fs.existsSync(instance));
const config = path.join(area, 'settings.json');
const work = path.join(area,'work'); fs.mkdirSync(work);
const parked = path.join(area,'parked'); fs.mkdirSync(parked);
fs.writeFileSync(config, JSON.stringify({language:'ja', keep_terminals:false,
  agent_hooks:{'Claude Code':'off','Codex CLI':'off','Gemini CLI':'off'},
  desks:[{name:'Tab check', id:'tab-check', folders:[
    {name:'First',cwd:area,tabs:[]},
    {name:'Here',cwd:work,keep_first:true,tabs:[{name:'Shell',id:'shell',command:'cmd.exe'}]},
    {name:'Stored',cwd:parked,parked:true,tabs:[]}
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
  const ws = new WebSocket(target.webSocketDebuggerUrl); sockets.push(ws);
  await new Promise((resolve,reject)=>{ws.onopen=resolve;ws.onerror=reject;});
  let next=0; const pending=new Map();
  ws.onmessage = e => { const m=JSON.parse(e.data); if(m.id) pending.get(m.id)?.(m); };
  const call = (method,params={}) => new Promise((resolve,reject)=>{
    const id=++next;
    const timer=setTimeout(()=>{pending.delete(id);reject(Error('No reply: '+method));},15000);
    pending.set(id,m=>{clearTimeout(timer);pending.delete(id);m.error?reject(Error(JSON.stringify(m.error))):resolve(m.result);});
    ws.send(JSON.stringify({id,method,params}));
  });
  const js=async expression=>{
    const r=await call('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true});
    if(r.exceptionDetails) throw Error(safeError(r.exceptionDetails.exception?.description || r.exceptionDetails.text));
    return r.result?.value;
  };
  await call('Page.enable');
  return {call,js};
}
let settings;
try {
  ps(['-Config',config,'-Work',work,'-Cdp','0']);
  const targets=async(name)=>{
    const file=path.join(instance,'localappdata/ShikishaTerm/webview2',name,'EBWebView/DevToolsActivePort');
    const port=Number(fs.readFileSync(file,'utf8').split(/\r?\n/)[0]);
    return (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).filter(t=>t.type==='page');
  };
  const board=await attach(await until(async()=>(await targets('shell'))[0],'board'));
  await until(()=>board.js('S?.tabs?.some(t=>t.id === "shell") && !!document.querySelector("#strip .snew")'),'tab plus');
  const savedFile=path.join(instance,'app/config/config.json');
  const saved=()=>JSON.parse(fs.readFileSync(savedFile,'utf8'));
  const before=fs.readFileSync(savedFile,'utf8');
  async function open() {
    await board.js('document.querySelector("#strip .snew").click()');
    settings=await attach(await until(async()=>(await targets('profiles/default')).find(t=>t.url.includes('addtab=')),'settings child'));
    await until(()=>settings.js('document.body.classList.contains("float") && framed?.kind === "addtab"'),'add tab dialog');
    check(await settings.js('sel.grp === 1 && desks[0].folders[sel.grp].name === "Here"'),'+ keeps the active folder instead of the first folder');
  }
  await open();
  await settings.js('document.querySelector("#floatbox [data-frame=cancel]").click()');
  await until(()=>board.js('!S.settings_open'),'cancel closes the dialog');
  check(fs.readFileSync(savedFile,'utf8') === before,'cancel writes nothing');
  await open();
  // Emulate the browser contract on ordinary HTTP: getRandomValues exists,
  // randomUUID does not. Run before the real page's first instruction.
  await settings.call('Page.addScriptToEvaluateOnNewDocument',{source:'Object.defineProperty(crypto,"randomUUID",{value:undefined});'});
  await settings.call('Page.reload');
  await until(()=>settings.js('typeof crypto.randomUUID === "undefined" && framed?.kind === "addtab"'),'dialog without crypto.randomUUID');
  check(true,'+ opens the dialog when randomUUID is unavailable');
  const uid=await settings.js('framed.t.uid');
  await settings.js('const command=document.querySelector("#floatbody input.mono"); command.value="cmd.exe"; command.dispatchEvent(new Event("input",{bubbles:true}));');
  const shots=path.join(root,'target/shots/add-tab'); fs.mkdirSync(shots,{recursive:true});
  fs.writeFileSync(path.join(shots,'native.png'),Buffer.from((await settings.call('Page.captureScreenshot',{format:'png'})).data,'base64'));
  await settings.js('document.getElementById("floatadd").click()');
  await until(()=>board.js('!S.settings_open && S.tabs.filter(t=>t.kind === "pty").length === 2'),'added shell arrives on board');
  const folders=saved().desks[0].folders;
  check(folders[1].tabs.length===2 && folders[0].tabs.length===0,'save adds exactly one tab in the chosen folder');
  check(folders[1].tabs[1].uid===uid,'saving keeps the identity created in the dialog');
  check(folders[1].keep_first && folders[2].parked,'saving a new tab preserves pin and archive flags');

  await open();
  const committed=fs.readFileSync(savedFile,'utf8');
  const draft=await settings.js('framed.t.uid');
  await settings.js('document.querySelector("#floatbox [data-frame=more]").click()');
  check(await settings.js(`!framed && !sel.global && desks[0].tabs[sel.tab].uid === ${JSON.stringify(draft)}`),'More settings keeps the same new tab');
  await settings.call('Runtime.evaluate',{expression:'closeSettings()',awaitPromise:false});
  await until(()=>settings.js('!!document.querySelector("dialog.confirm-box")'),'unsaved work confirmation');
  await settings.js('document.querySelector("dialog.confirm-box button.danger").click()');
  await until(()=>board.js('!S.settings_open'),'discard closes full settings');
  check(fs.readFileSync(savedFile,'utf8')===committed,'discarding More settings leaves the saved tabs and folder flags unchanged');
  console.log('All add-tab checks passed. Fixture: '+area);
} catch(e) {
  console.error(safeError(e)); process.exitCode=1;
} finally { for(const ws of sockets) ws.close(); ps(['-Stop']); }
