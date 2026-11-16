/**
 * A tab's notifications, on the tab's own page, for
 * tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-tab-notify.mjs
 *
 * One AI tab that tells a phone when it answers. What is judged: the
 * destination, the reply link's tick, and the tick of its own for being
 * told when the AI stops to ask something.
 */

const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';
const tabPage = '(async () => { sel = {desk:0, grp:0, tab:0, global:false}; render(); await ' + wait(500) + ';'
  + ' const boxes = Array.from(document.querySelectorAll("label")).filter(l => /notify|通知/i.test(l.textContent));'
  + ' const last = boxes[boxes.length - 1]; if (last) last.scrollIntoView({block:"center"}); })()';

export default {
  config: {
    desks: [{
      name: 'work', id: 'work',
      notify: { phone: { type: 'phone' } },
      folders: [{
        name: 'shop', cwd: 'D:/work/shop',
        tabs: [{ name: 'claude', id: 'claude', command: 'claude', notify_on_done: 'phone', notify_ask: true }],
      }],
    }],
  },
  scenes: {
    // The tab's page, at its notifications
    notify: tabPage,
  },
};
