// Exercise the real settings save code with delayed and repeated responses.
import fs from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const source = fs.readFileSync(new URL('../crates/core/src/webui.rs', import.meta.url), 'utf8');
const from = source.indexOf('let idsAtLoad = new Map();');
const to = source.indexOf('// Whether the saved language setting', from);
assert.ok(from >= 0 && to > from);
function page() {
  const asks = [];
  const replies = [];
  const c = vm.createContext({asks, replies, current:{}, desks:[], savedSnapshot:'', T:{},
    ensureIds:()=>{}, ensureWsIds:()=>{}, refreshSave:()=>{}, result:()=>{}, fill:()=>'',
    languageNeedsRestart:()=>false, framed:true, SHEET:false, returnOnSave:false, stayAfterSave:true,
    settingsApi:async (_url, p) => { asks.push(JSON.parse(JSON.stringify(p))); return await replies.shift(); }});
  vm.runInContext(`function payload() { return {out:{...current, desks}, files:[]}; }
    const snapshot = () => JSON.stringify(payload());
    ${source.slice(from, to)}`, c);
  return {c, asks, replies, run:code=>vm.runInContext(code,c)};
}
{
  const p = page();
  p.run(`desks = [{uid:'desk', id:'before', notify:{url:'@notify/before/url'}, tabs:[{uid:'tab',id:'first',key:'@ssh/before/first/key'}]}];
    current.extra = {nested:['@before.secret','literal @before.secret']}; idsAtLoad = idsNow(); desks[0].id = 'after'; desks[0].tabs[0].id = 'second';`);
  p.replies.push({ok:true,renamed:{'@notify/before/url':'@notify/after/url','@ssh/before/first/key':'@ssh/after/second/key','@before.secret':'@after.secret'}});
  assert.equal(await p.run('doSave()'), true);
  assert.equal(p.run('snapshot() === savedSnapshot'), true);
  p.replies.push({ok:true,renamed:{}});
  await p.run('doSave()');
  assert.equal(p.asks[1].config.desks[0].notify.url, '@notify/after/url');
  assert.equal(p.asks[1].config.desks[0].tabs[0].key, '@ssh/after/second/key');
  assert.deepEqual(p.asks[1].config.extra.nested, ['@after.secret', 'literal @before.secret']);
  assert.deepEqual(p.asks[1].moves, []);
  console.log('PASS consecutive saves keep committed secret references');
}
{
  const p = page();
  p.run(`desks = [{uid:'a',id:'a',notify:{url:'@notify/a/url'}},{uid:'b',id:'b',notify:{url:'@notify/b/url'}}];
    current.desks = desks; idsAtLoad = idsNow(); desks[0].id = 'b'; desks[1].id = 'a';`);
  p.replies.push({ok:true,renamed:{'@notify/a/url':'@notify/b/url','@notify/b/url':'@notify/a/url'}});
  await p.run('doSave()');
  assert.equal(p.run('desks[0].notify.url'), '@notify/b/url');
  assert.equal(p.run('desks[1].notify.url'), '@notify/a/url');
  console.log('PASS swapped references change once despite shared objects');
}
{
  const p = page();
  p.run(`desks = [{uid:'a',id:'old',notify:{url:'@notify/old/url'}}]; idsAtLoad = idsNow(); desks[0].id = 'new';`);
  p.replies.push({ok:false,error:'write failed'});
  assert.equal(await p.run('doSave()'), false);
  assert.equal(p.run('desks[0].notify.url'), '@notify/old/url');
  assert.ok(p.run('secretMovesOfNewIds().length') > 0);
  let done;
  p.replies.push(new Promise(resolve => {done = resolve;}));
  const saving = p.run('doSave()');
  p.run("desks[0].id = 'latest'");
  done({ok:true,renamed:{'@notify/old/url':'@notify/new/url'}});
  await saving;
  assert.equal(p.run('snapshot() === savedSnapshot'), false);
  assert.equal(p.run('secretMovesOfNewIds()[0][0]'), 'new.');
  assert.equal(p.run('secretMovesOfNewIds()[0][1]'), 'latest.');
  console.log('PASS failed saves and edits during a save retain their pending changes');
}
