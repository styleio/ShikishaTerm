/**
 * The sub-input bar a phone types into, for tools/debug/shoot.mjs. How a scene
 * file is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/composer.mjs
 *
 * This row is the phone's one door: an attachment, a backspace, the field
 * itself, Send, the ⌨ that hands typing to the screen instead, and the ✕. Six
 * things on a 390-pixel screen, and the field is the one that must keep its
 * room -- a row that grows a button at a time is how it stops being usable.
 *
 * What is judged: the buttons are one family (same size, same weight, same
 * quiet), the field is still the widest thing in the row, and nothing wraps or
 * slides under anything at the narrowest screen there is.
 */

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, state: 'BUSY', state_label: 'Working', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 1, 3, 2, 0, 0, 1, 0, 0, 0], group: 0, kind: 'pty',
  model: false, busy: true, settings: false, auto: true, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);

const state = (extra) => JSON.stringify(JSON.stringify(Object.assign({
  desk: 'work', desk_id: 'work', desks: ['work'], desk_index: 0, active: 1,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    { name: 'site', folder: 'D:/work/site', color: '#4285f4', linked: false,
      family: 'D:/work/site/.git', branch: 'main',
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs: [tab(0, 'claude', { ai: 'claude' }), tab(1, 'shell', { ai: null, auto: false })],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, remote_on: true, restartable: true, build: '',
  help_rows: [], ais: [],
}, extra)));

// The bar opens itself wherever there is something to type into, so putting a
// terminal tab in front is all a scene has to do. A draft is typed in as well:
// an empty field says nothing about whether there is room to read what you
// wrote with five buttons beside it
const typed = `window.__state(${state()});
  new Promise(r => setTimeout(() => {
    const i = document.getElementById("castinput");
    if (!i) { r(Promise.reject(new Error("the bar never opened"))); return; }
    i.value = "git switch -c fix/the-thing";
    i.dispatchEvent(new Event("input"));
    r("ok");
  }, 400))`;

export default {
  settle: 1200,
  served: 'remote',
  sizes: [['phone', 390, 820]],
  scenes: {
    // The row as it stands while something is being written into it
    typing: typed,
    // And with nothing in it, which is how it is first met
    empty: `window.__state(${state()}); "ok"`,
  },
};
