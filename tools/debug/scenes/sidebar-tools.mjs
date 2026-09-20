/**
 * The tools at the foot of the left bar, for tools/debug/shoot.mjs. How a scene
 * file is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/sidebar-tools.mjs
 *
 * Settings, the manual, the ideas and the tools stand in a row at the bottom of
 * the tab bar. They were pinned there by `margin-top:auto`, which only pins
 * while everything fits: on a day with enough projects and tabs to fill the
 * bar, the row became the last thing in a scrolling column and left the screen.
 * The list is longest exactly where those buttons are wanted most.
 *
 * So these scenes give the bar more than it can hold -- eight folders, tabs in
 * each -- and photograph it scrolled to the top, where the row must still be
 * standing at the floor with the rows passing under it. The drawer a phone
 * pulls out is the same bar and gets the same look.
 */

const fam = (n) => `D:/work/p${n}/.git`;
const tab = (index, name, group, extra) => Object.assign({
  index, name, id: name + index, state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 1, 3, 2, 0, 0, 1, 0, 0, 0], group, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);

// Eight projects with two tabs each: more than any of these screens can show
const groups = [];
const tabs = [];
for (let i = 0; i < 8; i++) {
  groups.push({
    name: ['site', 'api', 'docs', 'billing', 'mobile', 'infra', 'notes', 'sandbox'][i],
    folder: `D:/work/p${i}`, color: '#4285f4', linked: false, family: fam(i), branch: 'main',
    health: { as: 'fine' }, drift: { behind: 0, ahead: 0 },
  });
  tabs.push(tab(tabs.length, 'claude', i, { ai: 'claude' }));
  tabs.push(tab(tabs.length, 'shell', i));
}

const state = JSON.stringify(JSON.stringify({
  desk: 'work', desk_id: 'work', desks: ['work'], desk_index: 0, active: 1,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups, tabs,
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, remote_on: true, restartable: true, build: '', help_rows: [], ais: [],
}));

// Scrolled to the very top: the far end from where the row would have drifted
const atTop = `window.__state(${state});
  new Promise(r => setTimeout(() => {
    const bar = document.getElementById("tabs");
    bar.scrollTop = 0;
    const row = document.querySelector("#tabs .gearrow");
    if (!row) { r(Promise.reject(new Error("there is no row of tools"))); return; }
    r(bar.scrollHeight > bar.clientHeight ? "ok"
      : Promise.reject(new Error("the bar is not full, so this scene proves nothing")));
  }, 500))`;

export default {
  settle: 1500,
  scenes: {
    // The window: the bar holds more than it can show
    full: { run: atTop, sizes: [['wide', 1280, 860]] },
    // The same bar as a phone pulls it out
    drawer: {
      run: atTop + `.then(() => { document.getElementById("app").classList.add("drawer");
        return new Promise(r => setTimeout(() => r("ok"), 250)); })`,
      served: 'remote',
      sizes: [['phone', 390, 820]],
    },
  },
};
