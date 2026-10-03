/**
 * AIConfer with a crowded desk, for tools/debug/shoot.mjs: eight AI tabs,
 * each saying one line. What is judged is that no two faces wear the same
 * pair of colors, so every tab can be told from the others at a glance.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/confer-faces.mjs
 */

const now = Date.UTC(2026, 9, 1, 5, 30);
const names = ['otter', 'finch', 'heron', 'codex-b', 'claude-c', 'claude-a', 'gibbon', 'lynx'];
const deskUid = 'b7e166dc-1190-4347-ad31-2cf0b9235a11';

const tab = (index, name) => ({
  index, name, id: name, state: 'DONE', state_label: 'Done', profile: 'Claude Code',
  locked: false, depth: 0, activity: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0], group: 0, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: true, key: 'tab:' + index, ai: index % 2 ? 'claude' : 'codex',
});

const state = JSON.stringify({
  desk: 'work', desk_id: 'work', desk_uid: deskUid, desks: ['work'], desk_index: 0, active: 1,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [{ name: 'shop', folder: 'D:/work/shop', color: '#5b7cff', linked: false, family: 'D:/work/shop/.git', branch: 'main',
    health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } }],
  tabs: names.map((n, i) => tab(i + 1, n)),
  jobs: [],
  confer: { rev: 1, open: 0, open_desk: deskUid, auto_open: true, line_max: 80, max_rounds: 40 },
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
});

const said = names.map((n, i) => ({ k: 'line', id: i + 1, tab: n, at: now - (names.length - i) * 60000, how: 'aside', ask: null,
  marks: [], text: `This is ${n}.` }));

export default {
  setup: `window.__state(${JSON.stringify(state)}); setSideWidth(400); sideReveal("convo"); convoModeTo("confer"); "ok"`,
  scenes: {
    crowd: `window.__convo({panel: "confer", ok: true, req: CF.seq.confer_threads, act: "confer_threads", desk: "${deskUid}", tab: "otter", threads: [{id: 1, last_at: ${now}, tabs: ${JSON.stringify(names)}, first: "This is otter."}]});`
      + ` window.__convo({panel: "confer", ok: true, req: CF.seq.confer, act: "confer", desk: "${deskUid}", thread: 1, said: ${JSON.stringify(said)}, more: false});`
      + ` JSON.stringify(${JSON.stringify(names)}.map(n => cfPair(n).join("")))`,
  },
  langs: ['en'],
};
