/**
 * The working folder that is not on this machine, for tools/debug/shoot.mjs.
 * How a scene file is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/folder-not-here.mjs
 *
 * A desk carried over from another PC: one folder is here, one is on a drive
 * this machine has, and one is on a drive it has not. What is judged is
 * whether the card in the pane says what is wrong without jargon, whether the
 * one thing to press is obvious among the four, whether the breaking answer
 * keeps its distance, and whether the whole of it survives a phone's width.
 */

const site = 'D:/work/site/.git';
const tab = (index, name, group, extra) => Object.assign({
  index, name, id: name, state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0], group, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);

const groups = (hidden) => [
  { name: 'site', folder: 'D:/work/site', color: '#4285f4', linked: false, family: site,
    branch: 'main', health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  ...(hidden ? [] : [{ name: 'soj_main', folder: 'D:/server/soj_main', color: '#19c37d',
    linked: false, branch: 'main', health: { as: 'missing' }, drift: { behind: 0, ahead: 0 } }]),
  { name: 'notes', folder: 'X:/notes', linked: false,
    health: { as: 'nodrive', drive: 'X:' }, drift: { behind: 0, ahead: 0 } },
];

const state = (active, extra) => JSON.stringify(Object.assign({
  desk: 'work', desk_id: 'work', desks: ['work'], desk_index: 0, active,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: groups(false),
  tabs: [
    tab(0, 'shell', 0),
    tab(1, 'claude', 1, { ai: 'claude' }),
    tab(2, 'shell', 1),
    tab(3, 'shell', 2),
  ],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
}, extra || {}));

// The project's worktrees git knows and the desk does not list, each with the
// delete that empties the disk
const discovered = [{
  family: site,
  kept: false,
  found: [
    { folder: 'C:/Users/me/SHIKISHA-TERM/branches/site/login', branch: 'login' },
    { folder: 'C:/Users/me/SHIKISHA-TERM/branches/site/pricing-page', branch: 'pricing-page' },
    { folder: 'D:/old/site-hotfix', branch: 'hotfix' },
  ],
}];

export default {
  settle: 1500,
  scenes: {
    // The card itself: the folder of the tab in view is not on this machine
    card: `window.__state(${JSON.stringify(state(1))}); "ok"`,
    // The same for a drive this machine has not got: the one button that
    // cannot work is grey, and says why when it is pressed
    nodrive: `window.__state(${JSON.stringify(state(3))});`
      + ' document.querySelector("#held .hput").click(); "ok"',
    // The worktrees git knows and the desk does not list, opened out, each
    // line carrying its own delete
    found: `window.__state(${JSON.stringify(state(0, { discovered }))});`
      + ' foundOpen.add(' + JSON.stringify(site) + '); drawTabs(); "ok"',
    // Folders put away until the next launch: one line brings them all back
    hidden: `window.__state(${JSON.stringify(JSON.stringify({
      desk: 'work', desk_id: 'work', desks: ['work'], desk_index: 0, active: 0,
      hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
      groups: [groups(true)[0]],
      tabs: [tab(0, 'shell', 0)],
      ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
      auto_enabled: true, build: '', help_rows: [], ais: [], hidden: 2,
    }))}); "ok"`,
    // What will run, while it runs
    running: `window.__state(${JSON.stringify(state(1, {
      repair: {
        folder: 'D:/server/soj_main', name: 'soj_main',
        trouble: '', steps: [
          { do: 'clone', url: 'https://github.com/team/soj.git', to: 'D:/server/soj',
            line: 'git clone https://github.com/team/soj.git D:/server/soj' },
          { do: 'expand', branch: 'main', to: 'D:/server/soj_main',
            line: 'git -C D:/server/soj worktree add D:/server/soj_main main' },
        ],
        running: true, at_step: 0, done: false, asking: false, projects: [], branch: 'main',
      },
    }))}); openRepair({folder:"D:/server/soj_main", name:"soj_main",`
      + ' health:{as:"missing"}}); drawRepair(); "ok"',
  },
};
