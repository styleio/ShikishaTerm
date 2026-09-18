/**
 * The sidebar by project, for tools/debug/shoot.mjs. How a scene file is
 * written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/folder-cards.mjs
 *
 * Two git projects and a folder that is no repository. The first project has
 * its own checkout and two worktrees, with tabs in each; one set of tabs is put
 * away. What is judged is whether each folder reads as one group, whether the
 * folder in front can be told from the rest at a glance, and whether the cards
 * have room between them. The state is written the way the app sends it
 * (uistate.rs).
 */

const site = 'D:/work/site/.git';
const api = 'D:/work/api/.git';
const tab = (index, name, group, extra) => Object.assign({
  index, name, id: name, state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 1, 3, 2, 0, 0, 1, 0, 0, 0], group, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);

const state = (active) => JSON.stringify({
  desk: 'work', desk_id: 'work', desks: ['work'], desk_index: 0, active,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    { name: 'site', folder: 'D:/work/site', color: '#4285f4', linked: false, family: site, branch: 'main',
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
    { name: 'login', folder: 'C:/Users/me/SHIKISHA-TERM/branches/site/login', color: '#4285f4', linked: true,
      family: site, branch: 'login', health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
    { name: 'pricing-page', folder: 'C:/Users/me/SHIKISHA-TERM/branches/site/pricing-page', color: '#4285f4',
      linked: true, family: site, branch: 'pricing-page', health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
    // A worktree nothing has been started in yet: its card carries no tab, so
    // the row and the line under it are the only way in
    { name: 'search-box', folder: 'C:/Users/me/SHIKISHA-TERM/branches/site/search-box', color: '#4285f4',
      linked: true, family: site, branch: 'search-box', empty: true,
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
    { name: 'api', folder: 'D:/work/api', color: '#19c37d', linked: false, family: api, branch: 'main',
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
    { name: 'notes', folder: 'D:/notes', linked: false, health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs: [
    tab(0, 'claude', 0, { ai: 'claude', state: 'BUSY', state_label: 'Working', auto: true }),
    tab(1, 'shell', 0),
    tab(2, 'codex', 1, { ai: 'codex', state: 'DONE', state_label: 'Done' }),
    tab(3, 'shell', 1),
    tab(4, 'gemini', 2, { ai: 'gemini', state: 'QUESTION', state_label: 'Needs you' }),
    tab(5, 'claude', 4, { ai: 'claude' }),
    tab(6, 'shell', 5),
  ],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
});

export default {
  settle: 1500,
  scenes: {
    // The checkout's own folder in front
    primary: `window.__state(${JSON.stringify(state(0))}); "ok"`,
    // A worktree in front, one card down: the eye has to find it
    worktree: `window.__state(${JSON.stringify(state(2))}); "ok"`,
    // The worktree in front with its tabs brought out: the tab rows stand
    // inside the same box as their folder
    opened: `window.__state(${JSON.stringify(state(2))});`
      + ` putTabsAway("C:/Users/me/SHIKISHA-TERM/branches/site/login", false); "ok"`,
    // The worktree nothing runs in yet, with its project's other cards around
    // it: whether the card says on sight that it is waiting to be filled
    empty: `window.__state(${JSON.stringify(state(0))}); "ok"`,
    // The same on a phone, with the drawer the list lives in pulled out
    drawer: {
      run: `window.__state(${JSON.stringify(state(2))});`
        + ` putTabsAway("C:/Users/me/SHIKISHA-TERM/branches/site/login", false);`
        + ` document.getElementById("app").classList.add("drawer"); "ok"`,
      sizes: [['phone', 390, 820]],
    },
  },
};
