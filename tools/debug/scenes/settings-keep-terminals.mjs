/**
 * "Terminals on this PC", for tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-keep-terminals.mjs
 *
 * The row on the program's Basic page. What is judged is whether a file that
 * never chose shows the setting on, as the value it is, with the hint saying
 * it is the default and how to stop what keeps running; and, for a Microsoft
 * Store install, whether the line saying an update stops the terminals is
 * there. A copy served here is never a Store install, so the `store` scene
 * draws the row again the way a Store install draws it (`keepTerminalsRow(true)`)
 * and puts it in the place of the one drawn.
 */

const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';

const row = (store) => '(async () => { sel = {desk:null, grp:null, tab:null, global:true, section:"basic"}; render();'
  + ' await ' + wait(400) + ';'
  + (store ? ' const was = document.querySelector(".keeprow"); if (was) was.replaceWith(keepTerminalsRow(true));' : '')
  + ' const r = document.querySelector(".keeprow");'
  + ' if (r) r.scrollIntoView({block:"center"});'
  + ' await ' + wait(200) + '; })()';

export default {
  config: {
    desks: [{
      name: 'site', id: 'site',
      folders: [{ name: 'site', cwd: 'D:/work/site', tabs: [{ name: 'shell', command: 'cmd' }] }],
    }],
  },
  scenes: {
    // Never chosen: shown on
    keep: row(false),
    // A Store install: the line about updates is there too
    store: row(true),
  },
};
