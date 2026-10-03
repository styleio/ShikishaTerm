/**
 * The question about the SHIKISHA bridge on a machine added before the forms
 * had the box, for tools/debug/shoot.mjs. How a scene file is written is at
 * the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/bridge-offer.mjs
 *
 * What is judged: the machine is named in the title, what the bridge does is
 * one paragraph, what is put where and how it comes off is folded away, and
 * "put it there" is the main button. "taken" is the question asked again
 * after another question took the box over and was closed.
 */

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0], group: 0, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);
const key = '\u0001microvm(E2b)\u0001/home/user/site';
const state = JSON.stringify({
  desk: 'ops', desk_id: 'ops', desks: ['ops'], desk_index: 0, active: 1,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  hosts: [{ name: 'microvm(E2b)', at: '', kind: 'microvm' }],
  groups: [{ name: 'site', key, folder: '/home/user/site', host: 'microvm(E2b)', color: '#4285f4', linked: false,
    family: 'microvm(E2b):/home/user/site/.git', branch: 'main', health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } }],
  tabs: [tab(1, 'claude', { ai: 'claude' })],
  making: [],
  bridge_offer: { host: 'microvm(E2b)', kind: 'microvm' },
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
});

export default {
  settle: 1200,
  scenes: {
    asked: `window.__state(${JSON.stringify(state)}); "ok"`,
    taken: `window.__state(${JSON.stringify(state)});`
      + ` askQuestion({title: "another question", say: "", label: "OK", go: () => {}});`
      + ` window.__state(${JSON.stringify(state)}); closeAsk(true); window.__state(${JSON.stringify(state)}); "ok"`,
  },
};
