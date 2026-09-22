/**
 * The ✕ and what it costs, for tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-split.mjs
 *
 * Two rows on the program's Basic page, one under the other: "The ✕ button"
 * asks what closing the window costs, and "Screen and work" asks whether the
 * window is the program at all. What is judged is whether the pair reads as
 * two questions rather than one setting said twice, and whether the second
 * one says what it buys without talking about processes and allocators.
 */

const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';

// The program's own settings, scrolled to the pair
const basic = '(async () => { sel = {desk:null, grp:null, tab:null, global:true}; render();'
  + ' await ' + wait(400) + ';'
  + ' const row = [...document.querySelectorAll(".row .lbl, .row > :first-child")]'
  + '   .find(e => /✕|Screen and work|画面と作業/.test(e.textContent || ""));'
  + ' if (row) row.scrollIntoView({block:"center"});'
  + ' await ' + wait(200) + '; })()';

export default {
  config: {
    desks: [{
      name: 'site', id: 'site',
      folders: [{ name: 'site', cwd: 'D:/work/site', tabs: [{ name: 'shell', command: 'cmd' }] }],
    }],
  },
  scenes: {
    // As it arrives: the work stops with the window, and the window is the program
    off: basic,
    // Turned on, which is the state the wording has to make sense in
    on: '(async () => { await ' + basic + ';'
      + ' const box = [...document.querySelectorAll("input[type=checkbox]")]'
      + '   .find(b => /Screen and work|画面と作業|program of its own|別のプログラム/'
      + '     .test(b.closest(".row") ? b.closest(".row").textContent : ""));'
      + ' if (box && !box.checked) { box.click(); }'
      + ' await ' + wait(300) + '; })()',
  },
};
