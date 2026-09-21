/**
 * The row of controls a browser tab puts over its page, for
 * tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-browser-nav.mjs
 *
 * One browser tab, with some of the row ticked and some not. What is judged is
 * whether every button in that row is one tick with a name a person can read --
 * including click mode, which is drawn only for somebody watching from a phone
 * and has to say so where it is ticked.
 */

const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';
// The tab's own page, scrolled to the row of controls
const tabPage = (i) => '(async () => { sel = {desk:0, tab:' + i + ', global:false}; render();'
  + ' await ' + wait(500) + ';'
  + ' const rows = [...document.querySelectorAll(".card .row label")];'
  + ' const r = rows.find(l => l.nextElementSibling && l.nextElementSibling.querySelector("input[type=checkbox]"));'
  + ' if (r) r.scrollIntoView({block:"center"}); })()';

export default {
  config: {
    desks: [{
      name: 'site', id: 'site',
      folders: [{
        name: 'site', cwd: 'D:/work/site',
        tabs: [
          { name: 'page', id: 'page', command: 'browser https://example.com/',
            nav: { back: true, forward: true, reload: true, url: true } },
        ],
      }],
    }],
  },
  scenes: {
    // What was ticked, and what was left for the person to decide
    nav: tabPage(0),
  },
};
