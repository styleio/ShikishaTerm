/** Verify the complete deletion flow in an isolated app and disposable repository.
 * cargo build --bin SHIKISHA-TERM
 * node tools/debug/folder-removal.win.mjs
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';

const root = path.resolve(import.meta.dirname, '../..');
const fixture = fs.mkdtempSync(path.join(os.tmpdir(), 'sk-rm-'));
const main = path.join(fixture, 'main');
const work = path.join(fixture, 'topic');
const instance = path.join(fixture, 'instance');
assert.ok(path.resolve(instance).startsWith(path.resolve(fixture) + path.sep));
fs.mkdirSync(main);
const git = (...args) => {
  const ran = spawnSync('git', args, {cwd:main, encoding:'utf8'});
  assert.equal(ran.status, 0, ran.stderr);
  return ran.stdout.trim();
};
git('init', '-q', '-b', 'main');
git('config', 'user.name', 'Removal check');
git('config', 'user.email', 'removal@example.invalid');
fs.writeFileSync(path.join(main, 'saved.txt'), 'saved\n');
git('add', '.'); git('commit', '-qm', 'initial');
git('worktree', 'add', '-b', 'topic', work);
fs.writeFileSync(path.join(work, 'saved.txt'), 'staged\n');
git('-C', work, 'add', 'saved.txt');
fs.writeFileSync(path.join(work, 'saved.txt'), 'uncommitted\n');
fs.writeFileSync(path.join(work, 'new file.txt'), 'not tracked\n');
const head = git('rev-parse', 'topic');
const bulk = ['one', 'two'].map(name => path.join(fixture, name));
for (const folder of bulk) git('worktree', 'add', '-b', path.basename(folder), folder);
fs.writeFileSync(path.join(bulk[1], 'new.txt'), 'bulk review\n');
const config = path.join(fixture, 'settings.json');
fs.writeFileSync(config, JSON.stringify({language:'ja', confirm_worktree_delete:false,
  desks:[{id:'check', name:'Check', folders:[
    {name:'Main', cwd:main, tabs:[{id:'shell', name:'shell', command:'cmd.exe'}]},
    {name:'Topic', cwd:work, tabs:[]},
    ...bulk.map(cwd => ({name:path.basename(cwd), cwd, tabs:[]})),
  ]}]}));
const ps = args => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
  path.join(root, 'tools/debug/instance.win.ps1'), '-At', instance, ...args],
  {encoding:'utf8'});
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
let ws;
try {
  const started = ps(['-Work', main, '-Config', config]);
  assert.equal(started.status, 0, started.stdout + started.stderr);
  const address = started.stdout.match(/^cdp=(.*)$/m)?.[1].trim();
  assert.ok(address, started.stdout);
  let target;
  for (let n=0; n<80 && !target; n++) {
    try { target = (await (await fetch(address + '/json/list')).json()).find(t => t.type === 'page'); } catch {}
    if (!target) await sleep(250);
  }
  assert.ok(target, 'isolated window did not start');
  const cdp = await connectCdp(target); ws=cdp.ws;
  const {run} = cdp;
  const choose = `S.groups.find(g => g.folder.replaceAll('\\\\','/').toLowerCase() === ${JSON.stringify(work.replaceAll('\\', '/').toLowerCase())})`;
  for (let n=0; n<80; n++) {
    if (await run(`!!(S && S.groups && ${choose})`)) break;
    await sleep(250);
  }
  // Defer the app's unrelated hook proposal; never change the user's CLI
  // settings as part of a folder-removal check.
  await run(`if (S.hook_ask) send({kind:'agenthooks', answer:'later', seq:S.hook_ask.seq}); closeAsk(); 'deferred'`);
  await sleep(1000);
  const question = async () => {
    await run(`void discardFolder(${choose}); 'asked'`);
    for (let n=0; n<80; n++) {
      const shown = await run(`(async () => { await new Promise(r => setTimeout(r, 0)); const q=document.getElementById('sask'); return q && !q.hidden && q.querySelector('.vtitle').textContent === T['tui.discard.title'] ? {
        say:q.querySelector('.vsay').textContent, files:q.querySelector('.blist').textContent,
        never:q.querySelector('.snever').hidden, focused:document.activeElement.className,
        label:q.querySelector('.go').textContent} : null; })()`);
      if (shown) return shown;
      await sleep(250);
    }
    const diagnostic = await run(`({title:document.querySelector('#sask .vtitle')?.textContent,
      notice:document.querySelector('#toastmsg')?.textContent, waiting:checkingRemoval,
      folders:(S.groups || []).map(g => g.folder)})`);
    throw new Error('no removal review appeared: ' + JSON.stringify(diagnostic));
  };
  let shown = await question();
  assert.ok(shown.say.includes('2'), JSON.stringify(shown));
  assert.ok(shown.files.includes('saved.txt') && shown.files.includes('new file.txt'));
  assert.ok(shown.files.includes('topic'), 'kept branch must be named');
  assert.ok(shown.never, 'dirty-file consent must not be remembered');
  assert.ok(shown.focused.includes('quiet'), 'Cancel must have initial focus: ' + JSON.stringify(shown));
  await run(`document.activeElement.dispatchEvent(new KeyboardEvent('keydown', {key:'Enter', bubbles:true})); 'cancelled'`);
  await sleep(500);
  assert.ok(fs.existsSync(work), 'Enter on Cancel must preserve the folder');
  assert.ok(await run(`document.getElementById('sask').hidden`), 'Enter on Cancel must close the question');
  console.log('PASS dirty changes are reviewed even when clean confirmations are off; Enter cancels');

  await question();
  const view = await run(`(() => { const buttons=[...document.querySelectorAll('#sask .blist button')];
    const button=buttons.find(b => b.textContent === T['tui.discard.view_staged']);
    if (!button) return false; button.click(); return true; })()`);
  assert.ok(view, 'staged changes have their own view button');
  let editor;
  for (let n=0; n<60 && !editor; n++) {
    editor = await run(`(S.tabs || []).find(t => t.kind === 'editor') || null`);
    if (!editor) await sleep(250);
  }
  assert.ok(editor, 'review opens the existing editor');
  assert.ok(fs.existsSync(work), 'viewing changes must not delete anything');
  await run(`send({kind:'closetab', tab:${editor?.index || 0}, key:${JSON.stringify(editor?.key || '')}, sure:true}); true`);
  await sleep(1000);
  console.log('PASS staged changes open in the existing editor without deleting the folder');

  await question();
  fs.writeFileSync(path.join(work, 'new file.txt'), 'changed after confirmation appeared\n');
  await run(`document.querySelector('#sask .go').click(); 'confirmed'`);
  await sleep(2500);
  assert.ok(fs.existsSync(work), 'changed contents must invalidate the earlier approval');
  console.log('PASS a changed file invalidates the review before removal');

  await question();
  await run(`document.querySelector('#sask .go').click(); 'confirmed'`);
  for (let n=0; n<100 && fs.existsSync(work); n++) await sleep(250);
  assert.ok(!fs.existsSync(work), 'confirmed folder must be removed');
  assert.equal(git('rev-parse', 'topic'), head, 'branch and commits must remain');
  assert.ok(fs.existsSync(path.join(main, 'saved.txt')), 'main must remain');
  console.log('PASS confirmed changes are deleted, with main and committed branch history preserved');
  await sleep(500);
  await run(`void folderAction('delete', S.groups.filter(g => ${JSON.stringify(bulk.map(p => p.replaceAll('\\', '/').toLowerCase()))}.includes(g.folder.replaceAll('\\\\','/').toLowerCase()))); true`);
  let bulkShown = false;
  for (let n=0; n<80 && !bulkShown; n++) {
    bulkShown = await run(`(() => {const q=document.querySelector('#sask'); return q && !q.hidden && q.textContent.includes('new.txt');})()`);
    if (!bulkShown) await sleep(250);
  }
  assert.ok(bulkShown, 'bulk deletion must review each selected folder');
  await run(`document.querySelector('#sask .go').click(); true`);
  for (let n=0; n<120 && bulk.some(p => fs.existsSync(p)); n++) await sleep(250);
  assert.ok(bulk.every(p => !fs.existsSync(p)), 'both reviewed folders must be removed');
  assert.equal(git('rev-parse', 'one'), head);
  assert.equal(git('rev-parse', 'two'), head);
  console.log('PASS bulk removal checks every folder and preserves both branches');
} finally {
  ws?.close();
  ps(['-Stop']);
  console.log('Fixture: ' + fixture);
}
