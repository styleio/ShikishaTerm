import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const root = new URL('../../', import.meta.url);
const source = fs.readFileSync(new URL('crates/core/src/removal-review.js', root), 'utf8');
const shell = fs.readFileSync(new URL('crates/core/src/shell.rs', root), 'utf8');
const group = {key:'C:/work/topic', folder:'C:/work/topic'};
const clean = {ok:true, files:[], keeps_branch:true, branch:'topic', review:'current'};
const dirty = {...clean, files:[{path:'new.txt', untracked:true, staged:false, work:true}]};
function page(lang = 'en') {
  return vm.createContext({T:JSON.parse(fs.readFileSync(new URL(`lang/${lang}.json`, root))),
    el:(tag, props, ...children) => ({tag, props, children})});
}
async function review(answer, opts = {}, lang = 'en') {
  const ctx = page(lang);
  vm.runInContext(source, ctx);
  return ctx.reviewFolderRemoval([group], {request:async () => answer, ...opts});
}

test('skipping confirmations approves only a clean folder', async () => {
  let questions = 0;
  const question = async () => { questions++; return {unasked:true}; };
  await review(clean, {skip:true, question});
  assert.equal(questions, 0);
  const result = await review(dirty, {skip:true, question});
  assert.equal(questions, 1);
  assert.equal(result.unasked, false);
  assert.equal(result.reviews[group.key], 'current');
});
test('cancelling never returns permission to discard', async () => {
  assert.equal(await review(dirty, {question:async () => null}), null);
});
test('failed checks and unpublished machine commits cannot be approved', async () => {
  let asked = false;
  const question = async () => { asked = true; return {}; };
  await assert.rejects(review({ok:false, error:'Cannot read folder'}, {question}), /Cannot read folder/);
  await assert.rejects(review({...dirty, blocked:'Push these commits first'}, {question}), /Push these commits first/);
  assert.equal(asked, false);
});
for (const lang of ['en', 'ja']) {
  test(`the ${lang} question names the loss, files and branch being kept`, async () => {
    let spec;
    await review(dirty, {question:async s => { spec=s; return null; }}, lang);
    assert.ok(spec.dirty);
    assert.ok(spec.say.includes('1'));
    assert.ok(spec.label && !spec.label.includes('undefined'));
    const words = JSON.stringify(spec.rows);
    assert.ok(words.includes('topic') && words.includes('new.txt'));
  });
}
test('a multi-folder review carries a separate answer for each target', async () => {
  const ctx = page(); vm.runInContext(source, ctx);
  const result = await ctx.reviewFolderRemoval([group, {key:'second', folder:'second'}], {
    request:async (_, body) => ({...dirty, review:body.folder}), question:async () => ({}),
  });
  assert.equal(result.reviews[group.key], group.key);
  assert.equal(result.reviews.second, 'second');
});
test('cleanup takes precedence over an old remote branch only for a proven current HEAD', () => {
  const ctx = page();
  Object.assign(ctx, {G:{rows:[], branch:{name:'topic', upstream:'origin/topic', ahead:5, integrated_into:'origin/main'}},
    gitGroup:() => ({linked:true}), onMicrovm:() => false, gitStageable:() => [],
    gitPrsDone:() => true, gitPrsOpen:() => [], gitPrFormShown:() => false});
  vm.runInContext(shell.slice(shell.indexOf('function gitNext() {'), shell.indexOf('// Why a pull request cannot be made')), ctx);
  assert.equal(ctx.gitNext().icon, 'folder');
  delete ctx.G.branch.integrated_into;
  assert.equal(ctx.gitNext().icon, 'up', 'an old merged PR cannot hide commits added later');
  ctx.G.branch.integrated_into='main'; ctx.onMicrovm=() => true;
  assert.equal(ctx.gitNext().icon, 'up', 'destroying a machine also destroys its local repository');
});
