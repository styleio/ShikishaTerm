/**
 * The shortcuts screen, for tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-shortcuts.mjs
 *
 * The keys that work from any program -- the tools, the quick commands and the
 * ideas -- above the keys of this window. What is judged is whether the name
 * in the list says what the screen is, whether each row of the first list
 * reads as "this key opens this, from anywhere", and whether the quick commands
 * and the ideas are in that list and not in the second one. Served with no
 * window, so no key is registered: every row says so, and that is the state a
 * phone sees.
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
    // Out of the box
    shortcuts: '(async () => { sel = {desk:0, tab:null, global:true, section:"keys"}; render();'
      + ' await ' + wait(600) + '; window.scrollTo(0, 0); })()',
  },
};
