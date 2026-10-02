/**
 * Exercise saved folder flags, capacity and guarded bulk deletion in an isolated app.
 * Run: cargo build --bin SHIKISHA-TERM; node tools/debug/folder-management.win.mjs
 * Needs Windows, Node, PowerShell and git. Creates disposable repositories under
 * target, uses instance.win.ps1, and stops only that copy. No account is contacted.
 */
import fs from 'node:fs';
import path from 'node:path';
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
git(['add','.']); git(['commit','-qm','Start folder fixture']);
const names = ['clean','pinned','dirty','stored'];
for (const name of names) git(['worktree','add','-qb',name,path.join(area,name)]);
fs.writeFileSync(path.join(area,'dirty','unsaved.txt'),'keep me');
const folders = ['main',...names].map(name => ({name,cwd:path.join(area,name),tabs:[],keep_first:name === 'pinned'}));
fs.writeFileSync(config, JSON.stringify({language:'en', agent_hooks:{'Claude Code':'off','Codex CLI':'off','Gemini CLI':'off'}, desks:[{name:'Folder check',id:'folder-check',folders}]}));

let ws;
try {
  // The instance helper may replace -At recursively. Its resolved destination
  // is a new child of this fixture, and the repository fixtures are its siblings.
  const relative = path.relative(area, instance);
  if (relative !== 'instance' || fs.existsSync(instance)) throw Error('Unexpected instance destination');
  const output = ps(['-Config',config,'-Work',repo]);
  const cdp = output.match(/^cdp=(.+)$/m)?.[1].trim();
  if (!cdp) throw Error('The isolated instance did not return its DevTools address');
  const target = await until(async () => (await (await fetch(cdp + '/json/list')).json()).find(p => p.type === 'page'), 'board target');
  ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve,reject) => { ws.onopen = resolve; ws.onerror = reject; });
  let next = 0;
  const pending = new Map();
  ws.onmessage = event => { const message = JSON.parse(event.data); if (message.id) pending.get(message.id)?.(message); };
  const js = expression => new Promise((resolve,reject) => {
    const id = ++next;
    const timer = setTimeout(() => { pending.delete(id); reject(Error('No DevTools reply')); }, 15000);
    pending.set(id, message => {
      clearTimeout(timer); pending.delete(id);
      if (message.error || message.result?.exceptionDetails) reject(Error(JSON.stringify(message)));
      else resolve(message.result?.result?.value);
    });
    ws.send(JSON.stringify({id,method:'Runtime.evaluate',params:{expression,returnByValue:true,awaitPromise:true}}));
  });
  let state = await until(async () => {
    const s = await js('S'); return s?.folder_catalog?.length === 5 && s;
  }, 'saved folder catalog');
  const desk = state.desk_uid;
  const keys = Object.fromEntries(state.folder_catalog.map(g => [g.name,g.key]));
  const action = (act, list, workspace = desk) => js(`send(${JSON.stringify({kind:'foldermanage',desk:workspace,act,folders:list.map(n => keys[n])})})`);
  const saved = () => JSON.parse(fs.readFileSync(path.join(instance,'app/config/config.json'),'utf8')).desks[0].folders;
  await action('pin',['stored']);
  await until(async () => saved().find(f => f.name === 'stored')?.keep_first, 'pin persistence');
  await action('archive',['stored']);
  await until(async () => (await js('S')).folder_catalog.find(g => g.name === 'stored')?.parked, 'archive reload');
  assert(fs.existsSync(path.join(area,'stored','readme.txt')), 'archive preserves files');
  await action('restore',['stored']);
  await until(async () => !(await js('S')).folder_catalog.find(g => g.name === 'stored')?.parked, 'restore reload');
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
  await action('delete',['main','pinned','dirty','clean']);
  await until(async () => {
    const results = (await js('S')).folder_manage.results;
    return ['main','pinned','dirty','clean'].every(n => results[keys[n]] && !results[keys[n]].busy);
  }, 'all deletion outcomes', 60000);
  state = await js('S');
  assert(!fs.existsSync(path.join(area,'clean')) && !saved().some(f => f.name === 'clean'), 'clean worktree is removed from disk and settings');
  for (const name of ['main','pinned','dirty']) {
    assert(fs.existsSync(path.join(area,name,'readme.txt')) && !!state.folder_manage.results[keys[name]].error, `${name} is retained with a reason`);
  }
  assert(fs.readFileSync(path.join(area,'dirty','unsaved.txt'),'utf8') === 'keep me', 'uncommitted content survives bulk deletion');
  console.log('All folder management checks passed. Fixture: ' + area);
} finally {
  ws?.close();
  ps(['-Stop']);
}
