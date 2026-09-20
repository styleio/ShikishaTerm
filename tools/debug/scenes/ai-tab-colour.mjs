/**
 * Which tab wears its AI's colour, for tools/debug/shoot.mjs. How a scene file
 * is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/ai-tab-colour.mjs
 *
 * Four AIs side by side is the picture this app is for, and each one used to
 * wear its colour in the sidebar and along the top whether it was the tab being
 * looked at or not. Lit all at once, the colours said what selection says, and
 * the tab actually selected had nothing left to say it with.
 *
 * What is judged: exactly one row and one top tab are in colour, and they are
 * the selected one; the others read as the plain rows a terminal gets, with the
 * AI still named by its mark and its name; the status dots keep their own
 * meaning (green working, blue answered) on rows of either kind; and the colour
 * follows the selection from one AI to the next without the row moving.
 */

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 0, 1, 0, 0, 0, 0, 0, 0, 0], group: 0, kind: 'pty',
  model: false, busy: false, settings: false, auto: true, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);

// One desk, one folder, and the four CLIs a person is most likely to have
// running at once -- plus the terminal they are running beside, which is the
// row every unselected AI row is supposed to look like.
const tabs = [
  tab(0, 'claude', { ai: 'claude', state: 'BUSY', state_label: 'Working', busy: true }),
  tab(1, 'codex', { ai: 'codex', state: 'DONE', state_label: 'Answered' }),
  tab(2, 'gemini', { ai: 'gemini' }),
  tab(3, 'aider', { ai: 'aider', state: 'QUESTION', state_label: 'Asking' }),
  tab(4, 'shell', { ai: null, auto: false }),
];

const state = (extra) => JSON.stringify(JSON.stringify(Object.assign({
  desk: 'work', desk_id: 'work', desks: ['work'], desk_index: 0, active: 0,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    { name: 'site', folder: 'D:/work/site', color: '#4285f4', linked: false,
      family: 'D:/work/site/.git', branch: 'main',
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs,
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  // The build stamp has to match the page's, or the page decides it is out of
  // date and reloads itself out of the state it was just put into
  auto_enabled: true, remote_on: false, usage: null, restartable: true, build: '',
  help_rows: [], ais: [],
}, extra)));

// A folder keeps its tabs put away until somebody asks for them, and put away
// they are a row of chips rather than rows. The rows are the half of this that
// is being judged, so every scene opens the folder first.
const open = (active) => `window.__state(${state({ active })});`
  + ' putTabsAway("D:/work/site", false); "ok"';

export default {
  scenes: {
    // The working day: the AI being looked at in its colour, the three others
    // and the terminal quiet
    claude: open(0),
    // The same screen with another AI picked. Only the colour has moved --
    // nothing has changed width, and no row has changed place
    codex: open(1),
    // The terminal picked, so no AI is the selected tab: a sidebar with every
    // brand colour off, which must still read as a working board rather than
    // an empty or broken one
    shell: open(4),
    // Put away, which is the folder as it is first met: the chips keep the
    // colours, because a chip is not a tab and cannot be taken for the one
    // being looked at
    away: `window.__state(${state({ active: 0 })}); "ok"`,
    // A phone is served its own page, and the misreading this is about is
    // worst there: the strip is most of what there is room for
    phone: {
      run: open(0),
      served: 'remote',
      sizes: [['phone', 390, 820]],
    },
  },
};
