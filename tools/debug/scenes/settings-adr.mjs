/**
 * A project's ADR page, for tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-adr.mjs
 *
 * One project that keeps no records yet, in this checkout (the app answers
 * about it as a repository), and the same project after records are turned
 * on: the folder written as a value, never left blank to mean a default.
 */
import path from 'node:path';

const REPO = path.resolve(import.meta.dirname, '..', '..', '..').replaceAll('\\', '/');
const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';
const page = '(async () => { sel = {desk:0, proj:"p:shop", grp:null, tab:null, global:false, psection:"adr"};'
  + ' render(); await ' + wait(700) + '; window.scrollTo(0, 0); })()';

export default {
  config: {
    desks: [{
      name: 'Work', id: 'work',
      projects: [{ name: 'shop', at: REPO }],
      folders: [{ name: 'shop', cwd: REPO, project: 'shop', tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }],
    }],
  },
  scenes: {
    // Off: what an ADR is, and the box that turns it on
    off: page,
    // On: the folder, written out
    on: '(async () => { await ' + page + '; const b = document.getElementById("adr-on");'
      + ' b.checked = true; b.dispatchEvent(new Event("change")); await ' + wait(400) + '; window.scrollTo(0, 0); })()',
  },
};
