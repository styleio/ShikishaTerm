/** Real Git review over disposable folders, for tools/debug/settings-shoot.mjs. */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {execFileSync} from 'node:child_process';

const root = fs.mkdtempSync(path.join(os.tmpdir(), 'sk-review-'));
const main = path.join(root, 'main'), work = path.join(root, 'login');
fs.mkdirSync(main);
const git = (...args) => execFileSync('git', args, {cwd:main, stdio:'pipe'});
git('init', '-q', '-b', 'main');
git('config', 'user.name', 'Review check');
git('config', 'user.email', 'review@example.invalid');
fs.writeFileSync(path.join(main, 'settings.txt'), 'saved\n');
git('add', '.'); git('commit', '-qm', 'initial');
git('worktree', 'add', '-b', 'login', work);
fs.writeFileSync(path.join(work, 'settings.txt'), 'edited\n');
fs.writeFileSync(path.join(work, 'new file.txt'), 'new\n');

export default {
  config:{desks:[{id:'site', name:'Site', folders:[
    {name:'Site', cwd:main, tabs:[]}, {name:'Login', cwd:work, tabs:[]},
  ]}]},
  scenes:{removal:`(async () => {
    sel = {desk:0, grp:1, tab:null, global:false}; render();
    let button;
    for (let n=0; n<60 && !button; n++) {
      button = [...document.querySelectorAll('#detail button')].find(b => b.textContent === T['settings.group.discard']);
      if (!button) await new Promise(r => setTimeout(r, 100));
    }
    if (!button) throw new Error('Delete folder is missing');
    button.click();
    let dialog;
    for (let n=0; n<60 && !dialog; n++) {
      dialog = document.querySelector('dialog.confirm-box');
      if (!dialog) await new Promise(r => setTimeout(r, 100));
    }
    if (!dialog) throw new Error('The real review API did not show a question');
    if (!dialog.textContent.includes('new file.txt') || !dialog.textContent.includes('settings.txt')) throw new Error('Changed files are missing');
    if (dialog.querySelector('.mfoot .quiet') !== document.activeElement) throw new Error('Cancel must have focus: ' + document.activeElement.outerHTML);
    if (dialog.querySelector('.removal-files button')) throw new Error('Standalone settings must not offer a missing editor');
    return 'ok';
  })()`},
};
