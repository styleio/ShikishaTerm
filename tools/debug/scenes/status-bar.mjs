/**
 * The bar across the top of a phone (and along the foot of a window), for
 * tools/debug/shoot.mjs. How a scene file is written is at the top of
 * files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/status-bar.mjs
 *
 * What this bar carries is as long as the day makes it: the desk's name, AUTO,
 * REMOTE, the subscription's two windows, and the notice a CLI prints when it
 * has run out. On a phone that is several times the width. The three controls
 * -- the drawer, RESTART and STOP -- have to keep their places at the two
 * ends, and a phone is served its own page, so the scenes below ask for it.
 *
 * What is judged: the readings stop where the controls begin rather than
 * sliding under them, the row can be pulled sideways to read the rest, and a
 * bar with little in it looks no different for being pullable.
 */

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, state: 'BUSY', state_label: 'Working', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 1, 3, 2, 0, 0, 1, 0, 0, 0], group: 0, kind: 'pty',
  model: false, busy: true, settings: false, auto: true, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);

const usage = {
  title: '5h: 5% used \u00b7 resets in 1h 12m \u00b7 7d: 51% used \u00b7 resets in 17h 40m',
  five: { name: '5h', pct: 5, used: '5% used', resets: '1h 12m' },
  week: { name: '7d', pct: 51, used: '51% used', resets: '17h 40m' },
};

const state = (extra) => JSON.stringify(JSON.stringify(Object.assign({
  desk: 'work', desk_id: 'work', desks: ['work'], desk_index: 0, active: 0,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    { name: 'site', folder: 'D:/work/site', color: '#4285f4', linked: false,
      family: 'D:/work/site/.git', branch: 'main',
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs: [tab(0, 'claude', { ai: 'claude' }), tab(1, 'shell', { ai: null, auto: false })],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  // The build stamp has to match the page's, or the page decides it is out of
  // date and reloads itself out of the state it was just put into
  auto_enabled: true, remote_on: true, usage, restartable: true, build: '',
  help_rows: [], ais: [],
}, extra)));

// Pull the readings to their far end, the way a thumb would
const flick = ' new Promise(r => setTimeout(() => { const m = document.querySelector("#status .stmid");'
  + ' m.scrollLeft = m.scrollWidth; r("ok"); }, 200))';

export default {
  settle: 1500,
  served: 'remote',
  sizes: [['phone', 390, 820]],
  scenes: {
    // What the bar carries on an ordinary working day
    full: `window.__state(${state()}); "ok"`,
    // The same bar pulled to its far end: RESTART and STOP have not moved,
    // and the last of the reading has come into view
    scrolled: `window.__state(${state()});` + flick,
    // Pulled, and then left where the next state push finds it. This bar is
    // rebuilt several times a second, and a flick that snapped back to the
    // left on the next push would be a row nobody could read to the end
    kept: `window.__state(${state()});` + flick
      + `.then(() => { window.__state(${state()});`
      + ' return document.querySelector("#status .stmid").scrollLeft > 0'
      + ' ? "ok" : Promise.reject(new Error("the flick was lost on the next state")); })',
    // With the CLI's own notice as well, which is the longest thing the bar
    // ever carries
    limited: `window.__state(${state({
      tabs: [tab(0, 'claude', { ai: 'claude', limit: 'usage limit reached, resets 3pm' })],
    })}); "ok"`,
    // Nothing but the desk and AUTO: there is nothing to pull, and the bar
    // must not look any different for it
    quiet: `window.__state(${state({
      remote_on: false, usage: null,
      tabs: [tab(0, 'shell', { ai: null, auto: false })],
    })}); "ok"`,
    // The window's own bar, where the readings are cut short rather than
    // pulled: what must not change while the phone's is being fixed
    window: {
      run: `window.__state(${state()}); "ok"`,
      served: 'window',
      sizes: [['wide', 1280, 860], ['narrow', 900, 700]],
    },
  },
};
