/**
 * Keeping the PC awake, for tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-stay-awake.mjs
 *
 * The row on the program's Basic page. What is judged is whether a file that
 * never chose shows "Off" as the value it is (not an empty box), whether the
 * three choices read as three answers to one question, and whether the line
 * under them says what a laptop's lid does without talking about power APIs.
 */

const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';

const row = (config) => '(async () => { sel = {desk:null, grp:null, tab:null, global:true, section:"basic"}; render();'
  + ' await ' + wait(400) + ';'
  + ' const r = [...document.querySelectorAll(".row")].find(e => /Keep the PC awake|スリープ防止/.test(e.textContent || ""));'
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
    // Never chosen: shown as Off
    awake: row(),
  },
};
