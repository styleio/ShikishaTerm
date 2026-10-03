// Real conversation and code review cases for confer-real.win.mjs.
// Structural checks are automatic; the saved lines and full replies are
// reviewed together for meaning. No keyword count pretends to judge prose.
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';

function checkExchanges(rows,count) {
  assert.equal(rows.length,count,'expected number of asks');
  for(const r of rows) {
    assert.equal(r.state,'DONE');
    assert.ok(r.reply?.trim(),'the full answer is retained');
    for(const how of ['ask','said']) {
      const line=r.lines.find(l=>l.how===how);
      assert.ok(line,how+' line is present (a fallback is not a prompt evaluation)');
      assert.ok([...line.text].length<=80 && !/[\r\n]/.test(line.text));
    }
  }
}

// Identical full answers across builds isolate the line instruction from
// differences in the caller's request and the work the answering AI did.
export async function lines({door,exchanges,until,out}) {
  const samples=[
    ['quoted-title','「白鳥」、弾けたら素敵だね！私なら空中ブランコに挑戦したい。'],
    ['review-scope','documents.cjs の所有者チェックの修正を確認しました。このファイル内では残る指摘はありません。ただし user.id が認証済みの信頼できる値であることが前提です。認証基盤と他のAPIは未確認なので、システム全体が安全だとは判断していません。'],
    ['tests-not-run','権限チェックを追加しました。ただし依存パッケージが不足していてテストを実行できていません。コード上の修正までで、動作確認は未完了です。']
  ];
  const results=[];
  for(const [name,answer] of samples) {
    const since=Date.now();
    await door('ask_tab','codex-b','表示用の一言を確認します。',
      'This tests reply handling. Reply with exactly the text below, without tools or additions:\n'+answer);
    await until(async()=>exchanges(since).some(r=>r.lines.some(l=>l.how==='said')),'line for '+name,100000);
    const rows=exchanges(since);
    checkExchanges(rows,1);
    assert.equal(rows[0].reply,answer,'the full answer is unchanged');
    results.push({name,answer,line:rows[0].lines.find(l=>l.how==='said').text});
    fs.writeFileSync(path.join(out,'lines.json'),JSON.stringify(results,null,2));
    console.log('RECORDED '+name+': '+results.at(-1).line);
  }
}

export async function dialogue({door,state,screen,exchanges,until,sleep,out,folders,first,reverse}) {
  const cases=[];
  const save=()=>fs.writeFileSync(path.join(out,'dialogue.json'),JSON.stringify(cases,null,2));
  const trial=async (name,caller,prompt,count)=>{
    const since=Date.now();
    console.log('Starting '+name);
    await door('send_to_tab',caller,prompt);
    const result={name,caller,prompt,exchanges:[],pass:false}; cases.push(result);
    try {
      await until(async()=>{
        const rows=exchanges(since);
        if (!rows.length && /Login expired|Not logged in|Please run \/login/i.test(await screen(caller)))
          throw new Error('the caller needs to sign in');
        return rows.length>=count && rows.every(r=>r.state!=='waiting' && r.lines.some(l=>['said','auto'].includes(l.how)))
          && ['DONE','WAIT'].includes(await state(caller));
      },name,8*60000);
      await sleep(1500);
      result.exchanges=exchanges(since);
      result.screen=await screen(caller);
      checkExchanges(result.exchanges,count);
      result.pass=true;
      console.log('PASS '+name+' ('+count+' exchanges)');
      return result;
    } finally { result.exchanges=exchanges(since); save(); }
  };
  const caller=reverse?'codex-b':first, reviewer=reverse?first:'codex-b';
  const work=reverse?folders.b:folders.a;
  await trial('chat',caller,
    `Have a casual Japanese conversation with <@${reviewer}> about trying an ambitious new hobby. `+
    'Use ask_tab exactly five times, naturally following each reply. Share your own reactions and ask light questions; '+
    'close the conversation on the fifth exchange. No research, files or other tools except shikisha. End with a brief goodbye.',5);

  fs.writeFileSync(path.join(work,'documents.cjs'),
    'const docs = new Map([["alpha", {id:"alpha",ownerId:"alice",title:"Draft"}]]);\n'+
    'exports.readDocument = (user, id) => {\n'+
    '  const doc = docs.get(id);\n'+
    '  if (!doc) return {status:404};\n'+
    '  return {status:200, body:{id:doc.id,title:doc.title}};\n};\n');
  fs.writeFileSync(path.join(work,'check.cjs'),
    'const a=require("node:assert/strict"),{readDocument:r}=require("./documents.cjs");\n'+
    'a.equal(r({id:"alice"},"alpha").status,200);\n'+
    'a.ok([401,403,404].includes(r({id:"bob"},"alpha").status));\n'+
    'a.ok([401,403,404].includes(r(null,"alpha").status));\n'+
    'a.equal(r({id:"alice"},"missing").status,404);\n'+
    'a.equal(r({id:"alice"},"alpha").body.titleLength,5);\n'+
    'console.log("ACCESS-CHECKS-PASS");\n');
  const before=spawnSync(process.execPath,['check.cjs'],{cwd:work,encoding:'utf8',windowsHide:true});
  assert.notEqual(before.status,0,'the fixture begins with an access-control defect');
  const checkBefore=fs.readFileSync(path.join(work,'check.cjs'),'utf8');
  const code=await trial('code-review',caller,
    `In your folder, add titleLength to the response in documents.cjs. Then ask <@${reviewer}> via ask_tab to review that file for security. `+
    `Give the reviewer this absolute folder path: ${work}. Review scope is only documents.cjs; authentication middleware and other APIs are unavailable. `+
    'Fix any findings, run node check.cjs without modifying it, then ask the same reviewer once more to verify the fix. '+
    'Use ask_tab exactly twice. Reply in Japanese; work only in this test folder. Do not commit or push.',2);
  assert.equal(fs.readFileSync(path.join(work,'check.cjs'),'utf8'),checkBefore,'the AI did not weaken the checks');
  const checked=spawnSync(process.execPath,['check.cjs'],{cwd:work,encoding:'utf8',windowsHide:true});
  code.check={status:checked.status,stdout:checked.stdout,stderr:checked.stderr};
  code.source=fs.readFileSync(path.join(work,'documents.cjs'),'utf8'); save();
  assert.equal(checked.status,0,'owner, other user, anonymous and missing-document checks pass');
  console.log('PASS independent access checks');
}
