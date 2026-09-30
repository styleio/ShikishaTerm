/**
 * The AIConfer card of the program's settings, for tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-confer.mjs
 *
 * What is judged: a file that never chose shows both values as they are (on,
 * 80), and the lines under them say what happens without talking about hooks.
 */

const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';

export default {
  config: {
    desks: [{
      name: 'site', id: 'site',
      folders: [{ name: 'site', cwd: 'D:/work/site', tabs: [{ name: 'shell', command: 'cmd' }] }],
    }],
  },
  scenes: {
    confer: '(async () => { sel = {desk:null, grp:null, tab:null, global:true, section:"confer"}; render(); await ' + wait(400) + '; })()',
  },
};
