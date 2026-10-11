/**
 * How a phone reaches the right panel, for tools/debug/shoot.mjs. How a scene
 * file is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/phone-right-panel.mjs
 *
 * The panel used to be reached by a strip down the right edge of the screen,
 * and that strip lay over the end of the row of tabs, where the list of closed
 * tabs is: a press meant for one opened the other. The panel now has a handle
 * in the top bar's right corner, the twin of the drawer's on the left, and the
 * list of closed tabs stands beside +.
 *
 * What is judged: the handle and STOP are apart, with a rule between them; no
 * strip is left on the right edge; ▾ stands beside +, nowhere near the handle;
 * and the handle opens the panel, says it is open, and puts it away again.
 */

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 0, 1, 0, 0, 0, 0, 0, 0, 0], group: 0, kind: 'pty',
  model: false, busy: false, settings: false, auto: true, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);

const state = JSON.stringify(JSON.stringify({
  desk: 'work', desk_id: 'work', desks: ['work'], desk_index: 0, active: 0,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    { name: 'site', folder: 'D:/work/site', color: '#4285f4', linked: false,
      family: 'D:/work/site/.git', branch: 'main',
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs: [tab(0, 'claude', { ai: 'claude' }), tab(1, 'shell', { ai: null, auto: false })],
  closed: [{ id: 7, name: 'codex', folder: 'D:/work/site', at: 0 }],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  // The build stamp has to match the page's, or the page decides it is out of
  // date and reloads itself out of the state it was just put into
  auto_enabled: true, remote_on: true, usage: null, restartable: true, build: '',
  help_rows: [], ais: [],
}));

// The page as a phone first meets it, measured. Throws when what is judged
// is not so, so a picture of a broken screen is never taken for a good one
const settle = `window.__state(${state}); putTabsAway("D:/work/site", false);
  new Promise((ok, no) => setTimeout(() => {
    const box = id => document.getElementById(id).getBoundingClientRect();
    const handle = box("sidebtn"), stop = box("stop");
    if (!handle.width) return no(new Error("there is no handle for the right panel"));
    if (handle.left - stop.right < 12) return no(new Error("the handle is " + (handle.left - stop.right) + "px from STOP"));
    if (getComputedStyle(document.getElementById("sidegrip")).display !== "none") return no(new Error("the edge strip is still there"));
    const plus = document.querySelector("#strip .snew").getBoundingClientRect();
    const closed = document.querySelector("#strip .sclosed").getBoundingClientRect();
    if (Math.abs(closed.left - plus.right) > 1) return no(new Error("the closed tabs are not beside +"));
    if (innerWidth - closed.right < 100) return no(new Error("the closed tabs are still out in the corner"));
    ok("ok");
  }, 200))`;

// Pressed: the panel is out and the handle says so; pressed again, it goes
// The last statement of settle is its promise, so this hangs off it
const press = `${settle}.then(() => {
  const handle = document.getElementById("sidebtn"), side = document.getElementById("side");
  handle.click();
  if (side.hidden || !handle.classList.contains("on")) throw new Error("the handle did not open the panel");
  handle.click();
  if (!side.hidden || handle.classList.contains("on")) throw new Error("the handle did not put the panel away");
  handle.click();
  return "ok";
})`;

export default {
  scenes: {
    'phone-panel-away': { run: settle, served: 'remote', sizes: [['phone', 390, 820]] },
    'phone-panel-out': { run: press, served: 'remote', sizes: [['phone', 390, 820]] },
  },
};
