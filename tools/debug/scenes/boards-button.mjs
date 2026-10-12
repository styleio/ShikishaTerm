/**
 * The 🖥️ at the foot of the sidebar, for the server versions this PC was
 * paired with, for tools/debug/shoot.mjs. How a scene file is written is at
 * the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/boards-button.mjs
 *
 * None paired: no 🖥️ at all. One: the 🖥️, which opens it. Two: the 🖥️, and
 * the list it opens -- a row for each, and the settings' page at the end.
 * The state is written the way the app sends it (uistate.rs).
 */

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 1, 3, 2, 0, 0, 1, 0, 0, 0], group: 0, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);

const state = (boards) => JSON.stringify({
  desk: 'site', desk_id: 'site', desks: ['site'], desk_index: 0, active: 1,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [{ name: 'site', folder: 'D:/work/site', color: '#4285f4', linked: false,
    family: 'D:/work/site/.git', branch: 'main',
    health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } }],
  tabs: [tab(1, 'claude', { ai: 'claude' }), tab(2, 'shell')],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
  boards,
});

const show = (boards) => 'window.__state(' + JSON.stringify(state(boards)) + ');';

export default {
  settle: 1500,
  scenes: {
    none: show([]) + ' document.querySelector(".boardsbtn") ? "a button with nothing paired" : "ok"',
    one: show(['vps1']) + ' document.querySelector(".boardsbtn").title',
    two: show(['vps1', 'office']) + ' "ok"',
    list: show(['vps1', 'office']) + ' document.querySelector(".boardsbtn").click();'
      + ' [...document.querySelectorAll(".fmenu > div")].map(d => d.textContent).join(" | ")',
    // Many: the list scrolls inside itself, and its last row can be reached
    many: show(Array.from({ length: 30 }, (_, i) => 'server-' + (i + 1))) + ' document.querySelector(".boardsbtn").click();'
      + ' const m = document.querySelector(".fmenu"); m.scrollTop = m.scrollHeight;'
      + ' (m.getBoundingClientRect().bottom <= innerHeight && m.scrollHeight > m.clientHeight) ? "ok" : "the list runs off the window"',
    // A name with what a replacement would read as a pattern: shown as it is
    odd: show(['prod $&', 'b']) + ' document.querySelector(".boardsbtn").click();'
      + ' [...document.querySelectorAll(".fmenu > div")][0].textContent',
  },
};
