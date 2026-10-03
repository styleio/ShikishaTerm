/**
 * Exercise saved folder flags, capacity and guarded bulk deletion in an isolated app.
 * Run: cargo build --bin SHIKISHA-TERM; node tools/debug/folder-management.win.mjs
 * Needs Windows, Node, PowerShell and git. Creates disposable repositories under
 * target, uses instance.win.ps1, and stops only that copy. No account is contacted.
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {spawnSync} from 'node:child_process';

const root = path.resolve(import.meta.dirname, '../..');
const area = fs.mkdtempSync(path.join(root, 'target', 'folder-check-'));
const instance = path.join(area, 'instance');
const repo = path.join(area, 'main');
const config = path.join(area, 'settings.json');
const run = (command, args, cwd = root) => {
  const result = spawnSync(command, args, {cwd, encoding:'utf8', windowsHide:true});
  if (result.status !== 0) throw Error(result.stderr || result.stdout || `${command} failed`);
  return result.stdout;
};
const ps = args => run('powershell.exe', ['-NoProfile','-ExecutionPolicy','Bypass','-File',path.join(root,'tools/debug/instance.win.ps1'),'-At',instance,...args]);
const git = args => run('git', ['-c','user.name=Folder check','-c','user.email=check@example.invalid',...args], repo);
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const until = async (test, why, limit = 45000) => {
  const end = Date.now() + limit;
  while (Date.now() < end) { try { const answer = await test(); if (answer) return answer; } catch {} await sleep(250); }
  throw Error('Timed out: ' + why);
};
const assert = (ok, why) => { if (!ok) throw Error(why); console.log('PASS ' + why); };

fs.mkdirSync(repo);
git(['init','-q','-b','main']);
fs.writeFileSync(path.join(repo,'readme.txt'), 'kept content\n');
fs.mkdirSync(path.join(repo,'src'));
fs.writeFileSync(path.join(repo,'src','work.txt'), 'child folder content\n');
git(['add','.']); git(['commit','-qm','Start folder fixture']);
const names = ['clean','pinned','dirty','stored','shared'];
for (const name of names) git(['worktree','add','-qb',name,path.join(area,name)]);
const sharedContent = fs.readFileSync(path.join(area,'shared','src','work.txt'));
fs.writeFileSync(path.join(area,'dirty','unsaved.txt'),'keep me');
const folders = ['main',...names].map(name => ({name,cwd:path.join(area,name),tabs:[],keep_first:name === 'pinned'}));
const liveUid = crypto.randomUUID(), hiddenUid = crypto.randomUUID();
folders.find(f => f.name === 'stored').tabs = [{name:'Stored shell', id:'stored-shell', uid:liveUid, command:'cmd.exe'}];
Object.assign(folders.find(f => f.name === 'pinned'), {parked:true,
  tabs:[{name:'Stored shell',id:'hidden-shell',uid:hiddenUid,command:'cmd.exe'}]});
fs.writeFileSync(config, JSON.stringify({language:'en', agent_hooks:{'Claude Code':'off','Codex CLI':'off','Gemini CLI':'off'}, desks:[
  {name:'Folder check',id:'folder-check',folders},
  {name:'Other desk',id:'away',folders:[{cwd:path.join(area,'shared','src'),tabs:[{id:'away-shell',command:'cmd.exe'}]}]},
]}));

let ws, readState;
try {
  // The instance helper may replace -At recursively. Its resolved destination
  // is a new child of this fixture, and the repository fixtures are its siblings.
  const relative = path.relative(area, instance);
  if (relative !== 'instance' || fs.existsSync(instance)) throw Error('Unexpected instance destination');
  const output = ps(['-Config',config,'-Work',repo]);
  const cdp = output.match(/^cdp=(.+)$/m)?.[1].trim();
  if (!cdp) throw Error('The isolated instance did not return its DevTools address');
  const target = await until(async () => (await (await fetch(cdp + '/json/list')).json()).find(p => p.type === 'page'), 'board target');
  const connection = await connectCdp(target, {timeout:15000});
  ws = connection.ws;
  const js = connection.run;
  let state = await until(async () => {
    const s = await js('S'); return s?.folder_catalog?.length === folders.length && s;
  }, 'saved folder catalog');
  readState = () => js('({folders:S.folder_catalog, results:S.folder_manage.results, tabs:S.tabs.map(t => ({id:t.id,state:t.state,name:t.name}))})');
  const desk = state.desk_uid;
  const keys = Object.fromEntries(state.folder_catalog.map(g => [g.name,g.key]));
  const action = (act, list, workspace = desk) => js(`send(${JSON.stringify({kind:'foldermanage',desk:workspace,act,folders:list.map(n => keys[n])})})`);
  const saved = () => JSON.parse(fs.readFileSync(path.join(instance,'app/config/config.json'),'utf8')).desks[0].folders;
  let quietSince = 0;
  await until(async () => {
    const quiet = (await js('S')).tabs.some(t => t.id === 'stored-shell' && ['WAIT','DONE'].includes(t.state));
    if (!quiet) quietSince = 0;
    else if (!quietSince) quietSince = Date.now();
    // Observe a full quiet second so the shell's startup banner has finished.
    return quietSince && Date.now() - quietSince >= 1000;
  }, 'idle saved shell');
  await js('send({kind:"tabname",tab:S.tabs.find(t=>t.id==="stored-shell").index,name:"Live renamed"})');
  await until(() => saved().find(f=>f.name==='stored').tabs[0].name === 'Live renamed', 'the visible tab is renamed');
  assert(saved().find(f=>f.name==='pinned').tabs[0].name === 'Stored shell', 'an archived namesake is not renamed');
  await until(async () => (await js('S')).tabs.some(t=>t.uid===liveUid && t.name==='Live renamed'), 'the live UID keeps its new title');
  assert(saved().find(f=>f.name==='stored').tabs[0].uid === liveUid && saved().find(f=>f.name==='pinned').tabs[0].uid === hiddenUid, 'renaming preserves both tab identities');
  const switchDesk = async index => {
    await js('send({kind:"opendesk"})');
    await until(() => js('S.desk_open'), 'desk picker');
    await js('send('+JSON.stringify({kind:'menu',key:String(index+1)})+')');
    await until(() => js('S.desk_index==='+index), 'desk switch');
  };
  await switchDesk(1);
  await until(() => js('S.tabs.some(t=>t.id==="away-shell")'), 'another desk holds the child directory');
  await switchDesk(0);
  await js('send('+JSON.stringify({kind:'folderdiscard',folder:keys.shared,unasked:false})+')');
  await until(() => js('S.flash?.includes("Another desk")'), 'individual deletion reports shared child protection');
  assert(fs.readFileSync(path.join(area,'shared','src','work.txt')).equals(sharedContent), 'individual deletion preserves files used by another desk');
  await action('pin',['stored']);
  await until(async () => saved().find(f => f.name === 'stored')?.keep_first, 'pin persistence');
  await action('archive',['stored']);
  await until(async () => (await js('S')).folder_catalog.find(g => g.name === 'stored')?.parked, 'archive reload');
  await until(async () => !(await js('S')).tabs.some(t => t.id === 'stored-shell'), 'archive closes the saved shell');
  assert(fs.existsSync(path.join(area,'stored','readme.txt')), 'archive preserves files');
  assert(saved().find(f => f.name === 'stored').tabs[0].id === 'stored-shell', 'archive preserves the shell definition');
  await action('restore',['stored']);
  await until(async () => !(await js('S')).folder_catalog.find(g => g.name === 'stored')?.parked, 'restore reload');
  await until(async () => (await js('S')).tabs.some(t => t.id === 'stored-shell'), 'restore launches the saved shell');
  assert(saved().find(f => f.name === 'stored').keep_first, 'restore keeps pin state');
  await action('unpin',['stored']);
  await until(async () => !saved().find(f => f.name === 'stored').keep_first, 'unpin persistence');
  await action('archive',['stored'],'wrong-desk');
  await sleep(1000);
  assert(!saved().find(f => f.name === 'stored').parked, 'stale workspace request changes nothing');
  await action('measure',['stored']);
  await until(async () => (await js('S')).folder_manage.usage[keys.stored], 'capacity measurement');
  state = await js('S');
  assert(state.folder_manage.usage[keys.stored].bytes > 0 && !state.folder_manage.usage[keys.stored].partial, 'capacity returns a completed nonzero estimate');
  await action('delete',['main','pinned','dirty','clean','shared']);
  await until(async () => {
    const results = (await js('S')).folder_manage.results;
    return ['main','pinned','dirty','clean','shared'].every(n => results[keys[n]] && !results[keys[n]].busy);
  }, 'all deletion outcomes', 60000);
  state = await js('S');
  assert(!fs.existsSync(path.join(area,'clean')) && !saved().some(f => f.name === 'clean'), 'clean worktree is removed from disk and settings');
  for (const name of ['main','pinned','dirty','shared']) {
    assert(fs.existsSync(path.join(area,name,'readme.txt')) && !!state.folder_manage.results[keys[name]].error, `${name} is retained with a reason`);
  }
  assert(fs.readFileSync(path.join(area,'dirty','unsaved.txt'),'utf8') === 'keep me', 'uncommitted content survives bulk deletion');
  assert(fs.readFileSync(path.join(area,'shared','src','work.txt')).equals(sharedContent), 'bulk deletion preserves the other desk child files and folder');
  console.log('All folder management checks passed. Fixture: ' + area);
} catch (error) {
  if (readState) console.error(JSON.stringify(await readState(), null, 2));
  throw error;
} finally {
  ws?.close();
  ps(['-Stop']);
}
