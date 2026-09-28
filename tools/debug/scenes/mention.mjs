/**
 * The @ in the input bar, for tools/debug/shoot.mjs: the list of this desk's
 * other AI tabs, and the badges a pick leaves in the text. How a scene file is
 * written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/mention.mjs
 *
 * What is judged: the list opens over the bar, never on the words being
 * typed; it is ordered nearest first (the same folder, then the same project,
 * then the rest) and says where each tab is; two tabs with one name are told
 * apart; a shell tab and the tab in front are not offered. A badge sits
 * exactly under its own letters, is a chip rather than a state colour, and
 * the row keeps the field as its widest thing on a phone.
 */

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, state: 'DONE', state_label: 'Done', profile: 'Claude Code',
  locked: false, depth: 0, activity: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0], group: 0, kind: 'pty',
  model: false, busy: false, settings: false, auto: true, restartable: true,
  readable: true, key: 'tab:' + index,
}, extra);

const groups = [
  { name: 'site', folder: 'D:/work/site', color: '#4285f4', linked: false,
    family: 'D:/work/site/.git', branch: 'main', health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  { name: 'fix-login', folder: 'D:/work/site-fix-login', color: '#4285f4', linked: true,
    family: 'D:/work/site/.git', branch: 'fix-login', health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  { name: 'docs', folder: 'D:/work/docs', color: '#a06bff', linked: false,
    family: 'D:/work/docs/.git', branch: 'main', health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
];

const state = JSON.stringify(JSON.stringify({
  desk: 'work', desk_id: 'work', desks: ['work'], desk_index: 0, active: 1,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups,
  tabs: [
    tab(0, 'board', { ai: null, kind: 'board' }),
    tab(1, 'claude', { ai: 'claude', id: 'finch' }),
    tab(2, 'codex', { ai: 'codex', id: 'otter', profile: 'Codex CLI' }),
    tab(3, 'shell', { ai: null, id: 'shell', profile: 'GENERIC', auto: false }),
    tab(4, 'claude', { ai: 'claude', id: 'panda', group: 1 }),
    tab(5, 'gemini', { ai: 'gemini', id: 'heron', group: 2, profile: 'Gemini CLI' }),
  ],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, remote_on: true, restartable: true, build: '',
  help_rows: [], ais: [],
}));

// The bar is opened (the window summons it; a phone has it), and the @ pressed
const opened = (then) => `window.__state(${state});
  new Promise((r, j) => setTimeout(() => {
    try {
      showDock();
      const i = document.getElementById("castinput");
      if (!i) throw new Error("the bar never opened");
      ${then}
      r("ok");
    } catch (e) { j(e); }
  }, 400))`;

export default {
  settle: 1200,
  scenes: {
    // The list, opened by the @ button over an empty field
    list: opened(`castInput.focus(); openMentions(-1);`),
    // Typing "@co" narrows it
    typing: opened(`castInput.focus();
      castInput.value = "Ask @co";
      castInput.setSelectionRange(7, 7);
      openMentions(4);`),
    // What a pick leaves behind: two badges in a sentence
    badges: opened(`castInput.focus();
      mentions.set("@codex", "otter");
      mentions.set("@claude (fix-login)", "panda");
      castInput.value = "When you are done, ask @codex to review it, then tell @claude (fix-login) what changed";
      castInput.dispatchEvent(new Event("input"));`),
  },
};
