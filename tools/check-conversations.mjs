// Run the board's real conversation state code without a window or account.
// node tools/check-conversations.mjs
import fs from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
import {webcrypto} from 'node:crypto';

const source = fs.readFileSync(new URL('../crates/core/src/shell.rs', import.meta.url), 'utf8');
const between = (start, end) => source.slice(source.indexOf(start), source.indexOf(end, source.indexOf(start)));
const core = between('const CV = {', '// -- the words on a row');
const follow = between('function convoFollow()', '// ── AIConfer');
const conference = between('const CF = {', '// A pause this long')
  + between('function cfAsk(', '// What the conference needs');
const history = between('let cvAll = false;', '// Every road that opened the search')
  + between('function allFindNow(', '// The conference, this conversation, or every conversation.')
  + between('window.__vaultWhere = function', 'function resumeRows(');
const tab = uid => ({index:1, uid, id:'coder', name:'Coder', kind:'pty', ai:'codex', readable:true, state:'DONE', activity:[]});
const row = (text, at=1) => ({k:'say', who:'you', record:'record', at, text});
function viewer() {
  const sent = [];
  const context = vm.createContext({crypto:webcrypto, sent, send:v=>sent.push(v),
    document:{getElementById:()=>null}, localStorage:{getItem:()=>null},
    setTimeout:()=>1, clearTimeout:()=>{}, CONVO_KINDS:[], T:{},
    drawConvo:()=>{}, drawSide:()=>{}, sideReveal:()=>{}, drawHead:()=>{}, drawAllIfShown:()=>{},
    S:{desk_id:'work', desk_uid:'desk-a', active:1, tabs:[tab('tab-a')]}, cvFromAll:false,
  });
  context.window = context;
  vm.runInContext(core + '\n' + follow + '\n' + conference + '\n' + history, context);
  return {context, sent, run:code=>vm.runInContext(code,context),
    reply(request, extra={}) { context.__convo({panel:request.panel,act:request.act,req:request.args.req,ok:true,rows:[],older:null,...extra}); }};
}
let failed = 0;
function check(name, test) {
  try {test(); console.log('PASS '+name);}
  catch(e) {failed++; console.error('FAIL '+name+'\n'+e.message);}
}
check('same-name tabs follow their stable identities',()=>{
  const v=viewer(); v.run('convoFollow()');
  v.reply(v.sent.at(-1),{rows:[row('from A')]});
  v.context.S.tabs=[tab('tab-b')]; v.run('convoFollow()');
  assert.equal(v.run('CV.panel'),'tab-b');
  assert.equal(v.run('CV.rows.length'),0);
});
check('an earlier-page reply from a previous visit is rejected',()=>{
  const v=viewer(); v.run('convoReset("a"); convoAsk("page", {}, "older")');
  const old=v.sent.at(-1);
  v.run('convoReset("b"); convoReset("a"); convoRefresh()');
  v.reply(old,{rows:[row('stale')]});
  assert.equal(v.run('CV.rows.length'),0);
});
check('two viewers cannot consume each others broadcast replies',()=>{
  const a=viewer(),b=viewer();
  for(const v of [a,b]) v.run('convoReset("a"); convoRefresh()');
  b.reply(a.sent.at(-1),{rows:[row('other viewer')]});
  assert.equal(b.run('CV.rows.length'),0);
});
check('pinning two different rows accepts both acknowledgements',()=>{
  const v=viewer();
  v.context.rows=[row('first',1),row('second',2)];
  v.run('convoReset("a"); CV.rows=rows; convoAsk("mark",{record:"record",at:1,pin:true}); convoAsk("mark",{record:"record",at:2,pin:true})');
  for(const request of v.sent) v.reply(request,{record:'record',at:request.args.at,pin:true});
  assert.equal(v.run('CV.rows.every(r=>r.pin)'),true);
});
check('a newer request still rejects an older reply in the same slot',()=>{
  const v=viewer(); v.run('convoReset("a"); convoRefresh(); convoRefresh()');
  v.reply(v.sent[1],{rows:[row('new')]}); v.reply(v.sent[0],{rows:[row('old')]});
  assert.equal(v.run('CV.rows[0].text'),'new');
});
check('the conference clears speech when following another tab',()=>{
  const v=viewer();
  v.run('cfRefresh(); cfShow(1, false); CF.said=[{id:1,k:"line",text:"from A"}]');
  v.context.S.tabs=[{...tab('tab-b'),id:'other'}]; v.run('cfRefresh()');
  assert.equal(v.run('CF.said.length'),0);
});
check('the conference accepts the desk UID returned by its reader',()=>{
  const v=viewer(); v.run('cfRefresh()');
  assert.equal(v.sent.at(-1).args.tab,'tab-a');
  v.reply(v.sent.at(-1),{desk:'desk-a',tab:'tab-a',threads:[{id:1}]});
  assert.equal(v.run('CF.thread'),1);
  assert.equal(v.sent.at(-1).act,'confer');
  v.reply(v.sent.at(-1),{desk:'desk-a',thread:1,said:[{k:'line',id:1,text:'Review complete'}],more:false});
  assert.equal(v.run('CF.said[0].text'),'Review complete');
  assert.equal(v.run('CF.loading'),false);
});
check('renaming a desk keeps its chosen conference',()=>{
  const v=viewer();
  v.run('cfRefresh(); cfShow(1,true); CF.said=[{k:"line",id:1,text:"Still here"}]');
  v.context.S.desk_id='renamed'; v.run('cfRefresh()');
  assert.equal(v.run('CF.thread'),1);
  assert.equal(v.run('CF.said[0].text'),'Still here');
});
check('a chosen conference does not stay visible in another desk',()=>{
  const v=viewer();
  v.run('cfRefresh(); cfShow(1, true); CF.said=[{id:1,k:"line",text:"from A"}]');
  const old=v.sent.at(-1);
  // A different desk can reuse the same readable ID.
  v.context.S.desk_uid='desk-b'; v.run('cfRefresh()');
  v.reply(old,{desk:'desk-a',thread:1,said:[{k:'line',id:2,text:'Late from A'}]});
  assert.equal(v.run('CF.thread'),null);
  assert.equal(v.run('CF.said.length'),0);
});
check('an earlier conference page from a previous visit is rejected',()=>{
  const v=viewer(); v.run('cfRefresh(); cfShow(1,false); cfAsk("confer",{thread:1},"earlier")');
  const old=v.sent.at(-1);
  v.run('cfShow(2,false); cfShow(1,false)');
  v.reply(old,{desk:'desk-a',thread:1,said:[{k:'line',id:1,text:'stale'}],more:false});
  assert.equal(v.run('CF.said.length'),0);
});
check('a reply delivered twice does not duplicate an older page',()=>{
  const v=viewer(); v.run('convoReset("a"); convoAsk("page",{},"older")');
  const request=v.sent.at(-1);
  v.reply(request,{rows:[row('once')]}); v.reply(request,{rows:[row('once')]});
  assert.equal(v.run('CV.rows.length'),1);
});
check('reading on replaces an unfinished answer that became tool work',()=>{
  const v=viewer();
  v.context.rows=[{...row('thinking',20),who:'ai'},row('request',0)];
  v.run('convoReset("a"); CV.rows=rows; CV.newer={record:"record",to:0,end:false}; convoNewer()');
  v.reply(v.sent.at(-1),{act:'newer', rows:[{...row('answer',50),who:'ai'},
    {k:'work',record:'record',from:20,to:50},row('request',0)],newer:{record:'record',to:0,end:true}});
  assert.equal(v.run('CV.rows.some(r=>r.text==="thinking")'),false);
});
check('all-history searches belong to the viewer that asked',()=>{
  const a=viewer(),b=viewer();
  a.run('CV.q="alpha"; allFindNow()'); b.run('CV.q="beta"; allFindNow()');
  b.reply(b.sent.at(-1),{vault:{query:'beta',hits:[{id:'mine'}]}});
  b.reply(a.sent.at(-1),{vault:{query:'alpha',hits:[{id:'other'}]}});
  assert.equal(b.run('cvAllState.query'),'beta');
  assert.equal(b.run('cvAllState.hits[0].id'),'mine');
});
check('a previous all-history query cannot replace a newer result',()=>{
  const v=viewer(); v.run('CV.q="old"; allFindNow()'); const old=v.sent.at(-1);
  v.run('CV.q="new"; allFindNow()');
  v.reply(old,{vault:{query:'old',hits:[{id:'old'}]}});
  assert.equal(v.run('cvAllState.query'),'new');
});
check('resume-folder replies stay with the viewer and current conversation',()=>{
  const a=viewer(),b=viewer();
  for(const v of [a,b]) v.run('convoReset("a"); cvWhereReq=convoRequest(CV.seq,"where")');
  b.context.__vaultWhere({req:a.run('cvWhereReq'),folder:'other'});
  assert.equal(b.run('cvWhere'),null);
  const req=b.run('cvWhereReq'); b.run('convoReset("b")');
  b.context.__vaultWhere({req,folder:'stale'});
  assert.equal(b.run('cvWhere'),null);
});
if(failed) process.exitCode=1;
