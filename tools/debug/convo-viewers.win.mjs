// Two viewers searching real, isolated conversation records through the app.
// No account, visible window, or live app settings. Build SHIKISHA-TERM first.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import http from 'node:http';
import {randomBytes} from 'node:crypto';
import {spawn} from 'node:child_process';
import assert from 'node:assert/strict';

const root = path.resolve(import.meta.dirname, '../..');
const run = fs.mkdtempSync(path.join(os.tmpdir(), 'sk-convo-viewers-'));
const instance = path.join(run, 'copy');
const profile = path.join(run, 'profile');
const work = path.join(run, 'work');
const records = path.join(profile, '.codex/sessions/2026/10/03');
for (const dir of [profile, work, records]) fs.mkdirSync(dir, {recursive:true});
for (const word of ['apple', 'banana']) {
  const id = word === 'apple' ? '11111111-1111-4111-8111-111111111111' : '22222222-2222-4222-8222-222222222222';
  const rows = [{type:'session_meta',payload:{id,session_id:id,cwd:work}},
    {type:'response_item',timestamp:'2026-10-03T00:00:00Z',payload:{type:'message',role:'user',content:[{type:'input_text',text:word+' orchard'}]}}];
  fs.writeFileSync(path.join(records, `rollout-2026-10-03T00-00-00-${id}.jsonl`), rows.map(r=>JSON.stringify(r)+'\n').join(''));
}
const ps = args => new Promise((resolve,reject)=>{
  const child=spawn('powershell.exe',['-NoProfile','-ExecutionPolicy','Bypass',...args],
    {windowsHide:true,env:{...process.env,USERPROFILE:profile}});
  let output='';
  child.stdout.on('data',d=>output+=d); child.stderr.on('data',d=>output+=d);
  child.on('error',reject); child.on('exit',code=>code===0?resolve(output):reject(new Error(output)));
});
const launcher=path.join(root,'tools/debug/instance.win.ps1');
let socket;
const received=[];
try {
  console.log('Starting an isolated app; '+run);
  const started=await ps(['-File',launcher,'-At',instance,'-Work',work,'-Cdp','0']);
  const base=/board=(http:\/\/127\.0\.0\.1:\d+)/.exec(started)?.[1];
  assert.ok(base,'the launcher named its board');
  const token=fs.readFileSync(path.join(instance,'app/data/remote-token'),'utf8').trim();
  const opened=await fetch(base+'/?t='+encodeURIComponent(token),{redirect:'manual'});
  await opened.text();
  const cookie=opened.headers.getSetCookie().map(c=>c.split(';')[0]).join('; ');
  assert.ok(cookie,'a viewer paired with the test app');
  // The state socket needs the paired cookie. Read the server's unmasked
  // text frames; the test never sends input over this socket.
  await new Promise((resolve,reject)=>{
    const req=http.get(base+'/ws-state',{headers:{Cookie:cookie,Connection:'Upgrade',Upgrade:'websocket',
      'Sec-WebSocket-Version':'13','Sec-WebSocket-Key':randomBytes(16).toString('base64')} });
    req.on('error',reject); req.on('response',r=>reject(new Error('state socket: '+r.statusCode)));
    req.on('upgrade',(_res,s,head)=>{
      socket=s;
      let bytes=head;
      const take=chunk=>{
        bytes=Buffer.concat([bytes,chunk]);
        while(bytes.length>=2) {
          const op=bytes[0]&15; let n=bytes[1]&127, off=2;
          if(n===126){if(bytes.length<4)return; n=bytes.readUInt16BE(2);off=4;}
          else if(n===127){if(bytes.length<10)return;n=Number(bytes.readBigUInt64BE(2));off=10;}
          if(bytes.length<off+n)return;
          const body=bytes.subarray(off,off+n);bytes=bytes.subarray(off+n);
          if(op===1){try{received.push(JSON.parse(body.toString()));}catch{}}
        }
      };
      s.on('data',take); take(Buffer.alloc(0)); resolve();
    });
  });
  const send=async o=>{
    const res=await fetch(base+'/api/intent',{method:'POST',headers:{Cookie:cookie,'content-type':'application/json'},body:JSON.stringify(o)});
    assert.equal(res.status,200); assert.equal((await res.json()).ok,true);
  };
  const waitFor=async (test,what)=>{
    const end=Date.now()+20000;
    while(Date.now()<end){if(test())return;await new Promise(r=>setTimeout(r,50));}
    throw new Error('Timed out: '+what);
  };
  const asks=['apple','banana'].map((q,i)=>({kind:'convo',panel:'vault',act:'find',
    args:{q,viewer:'viewer-'+i,req:'all#viewer-'+i+':1'}}));
  await Promise.all(asks.map(send));
  for(const ask of asks) {
    const got=()=>received.map(m=>m.convo).find(d=>d?.req===ask.args.req && d.vault && !d.vault.searching && !d.vault.asking);
    await waitFor(got,'search '+ask.args.q);
    const state=got().vault;
    assert.equal(state.query,ask.args.q);
    assert.ok(state.hits.length>0,'the isolated record was found');
    assert.ok(state.hits.every(h=>h.snippet.toLowerCase().includes(ask.args.q)),'the other query did not enter these results');
    console.log('PASS independent '+ask.args.q+' search');
  }
  const hit=received.map(m=>m.convo).find(d=>d?.req===asks[0].args.req && d.vault?.hits.length)?.vault.hits[0];
  await send({kind:'vaultwhere',program:hit.program,id:hit.id,req:'where#viewer-0:2'});
  const where=()=>received.map(m=>m.vaultwhere).find(d=>d?.req==='where#viewer-0:2');
  await waitFor(where,'resume location');
  assert.equal(where().ok,true,JSON.stringify({hit,reply:where()}));
  assert.equal(path.resolve(where().folder),path.resolve(work));
  console.log('PASS the resume location echoes its viewer request');
  const mark=async (hit,req,change)=>send({kind:'convo',panel:'past:'+hit.id,act:'mark',args:{
    req,record:hit.id,at:hit.at,past:{program:hit.program,id:hit.id,host:''},...change}});
  const hits=asks.map(ask=>received.map(m=>m.convo).find(d=>d?.req===ask.args.req && d.vault?.hits.length).vault.hits[0]);
  await Promise.all(hits.map((h,i)=>mark(h,'mark#pin-'+i,{pin:true})));
  await waitFor(()=>['mark#pin-0','mark#pin-1'].every(req=>received.some(m=>m.convo?.req===req && m.convo.ok)), 'both pins');
  const marks=()=>JSON.parse(fs.readFileSync(path.join(instance,'app/config/conversation-marks.json'),'utf8')).marks;
  assert.equal(marks().filter(m=>m.pinned).length,2);
  console.log('PASS two pins are both persisted');
  for(let n=0;n<25;n++) await mark(hit,'mark#note-'+n,{note:'ordered-note-'+n});
  await waitFor(()=>received.some(m=>m.convo?.req==='mark#note-24' && m.convo.ok), 'the latest note');
  assert.ok(marks().some(m=>m.note==='ordered-note-24'));
  console.log('PASS rapid note edits persist the final input');
} finally {
  socket?.destroy();
  await ps(['-File',launcher,'-At',instance,'-Stop']);
}
