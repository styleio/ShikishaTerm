/**
 * The question a server over SSH raises when it answers with a key other than
 * the one remembered for it, for tools/debug/shoot.mjs. How a scene file is
 * written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/host-key.mjs
 *
 * What is judged: the server is named, both fingerprints are there whole and
 * readable against what the server's owner says (not cut at the edge, not
 * wrapped mid-word into something else), the words say when to trust it and
 * when not to, the button that trusts it is the dangerous kind, and a phone
 * gets the same question it can answer with a finger.
 */

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0], group: 0, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);

const state = (extra) => JSON.stringify(Object.assign({
  desk: 'ops', desk_id: 'ops', desks: ['ops'], desk_index: 0, active: 0,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    { name: 'api', folder: 'D:/work/api', color: '#4285f4', linked: false,
      family: 'D:/work/api/.git', branch: 'main',
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs: [tab(0, 'prod')],
  making: [],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
}, extra));

// A server behind a bastion, the case the key used to be confused in
const change = {
  machine: 'gw-prod.example.com:22>10.0.0.1:22',
  before: 'SHA256:2Fq8mXk1v0pR6uN3yT9eLbZcW4hQ7sJdA5gKoVfIi0E',
  now: 'SHA256:yH7tN2cV9bXq1LmK4pW8sR3eZ6uJ0aDfG5oTiQ1vBnM',
};

const asked = `window.__state(${JSON.stringify(state({ key_changes: [change] }))});
  new Promise(r => setTimeout(() => r(document.getElementById("sask").hidden
    ? Promise.reject(new Error("nothing was asked")) : "ok"), 300))`;

const first = `window.__state(${JSON.stringify(state({ key_changes: [{...change, before: ''}] }))});
  new Promise(r => setTimeout(() => r(document.getElementById("sask").hidden
    ? Promise.reject(new Error("first key was not asked about")) : "ok"), 300))`;

export default {
  settle: 1200,
  scenes: {
    first,
    first_phone: { run: first, served: 'remote', sizes: [['phone', 390, 820]] },
    asked,
    phone: {
      run: asked,
      served: 'remote',
      sizes: [['phone', 390, 820]],
    },
  },
};
