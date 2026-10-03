/**
 * One project on several machines, for tools/debug/shoot.mjs. How a scene file
 * is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/project-machines.mjs
 *
 * "site" is checked out on this PC, on a MicroVM and on a server, with a
 * worktree on the first two; "api" lives on this PC alone, and "docs" on a
 * MicroVM alone. What is judged is whether "site" reads as one project with a
 * part per machine -- this PC first, then the MicroVM, then the server -- and
 * never as three projects side by side; whether "api" looks as it always did;
 * and whether "docs" says it is not on this PC. Then the dialog that asks
 * where a project is on a server that has no checkout of it, opened over the
 * worktree dialog. The state is written the way the app sends it (uistate.rs).
 */

const site = 'D:/work/site/.git';
const api = 'D:/work/api/.git';
const vmSite = 'devbox:/home/user/site/.git';
const srvSite = 'staging:/srv/site/.git';
const vmDocs = 'devbox:/home/user/docs/.git';
const fine = { health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } };
const tab = (index, name, group, extra) => Object.assign({
  index, name, id: name, state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 1, 3, 2, 0, 0, 1, 0, 0, 0], group, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);
const far = (host, at) => '\u0001' + host + '\u0001' + at;

const state = JSON.stringify({
  desk: 'work', desk_id: 'work', desks: ['work'], desk_index: 0, active: 0,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  hosts: [
    { name: 'devbox', at: '', kind: 'microvm' },
    { name: 'staging', at: 'ssh://deploy@staging.example.com:22', kind: 'ssh' },
  ],
  groups: [
    { name: 'site', key: 'D:/work/site', folder: 'D:/work/site', color: '#4285f4', linked: false,
      family: site, whole: 'site', project: 'site', branch: 'main', ...fine },
    { name: 'login', key: 'D:/work/site.worktrees/login', folder: 'D:/work/site.worktrees/login', color: '#4285f4',
      linked: true, family: site, whole: 'site', project: 'site', branch: 'login', ...fine },
    { name: 'api', key: 'D:/work/api', folder: 'D:/work/api', color: '#19c37d', linked: false,
      family: api, whole: 'api', project: 'api', branch: 'main', ...fine },
    { name: 'site', key: far('devbox', '/home/user/site'), folder: '/home/user/site', color: '#4285f4',
      linked: false, family: vmSite, whole: 'site', project: 'site', host: 'devbox', branch: 'main', ...fine },
    { name: 'search-box', key: far('devbox', '/home/user/site-search-box'), folder: '/home/user/site-search-box',
      color: '#4285f4', linked: true, family: vmSite, whole: 'site', project: 'site', host: 'devbox',
      branch: 'search-box', ...fine },
    { name: 'site', key: far('staging', '/srv/site'), folder: '/srv/site', color: '#4285f4', linked: false,
      family: srvSite, whole: 'site', project: 'site', host: 'staging', branch: 'main', empty: true, ...fine },
    { name: 'docs', key: far('devbox', '/home/user/docs'), folder: '/home/user/docs', color: '#a142f4',
      linked: false, family: vmDocs, whole: 'docs', project: 'docs', host: 'devbox', branch: 'main', ...fine },
  ],
  tabs: [
    tab(0, 'claude', 0, { ai: 'claude', state: 'BUSY', state_label: 'Working' }),
    tab(1, 'codex', 1, { ai: 'codex', state: 'DONE', state_label: 'Done' }),
    tab(2, 'claude', 2, { ai: 'claude' }),
    tab(3, 'claude', 3, { ai: 'claude', state: 'QUESTION', state_label: 'Needs you' }),
    tab(4, 'claude', 4, { ai: 'claude', state: 'BUSY', state_label: 'Working' }),
    tab(5, 'shell', 6),
  ],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
});

// The worktree dialog on "site", then the question over it about the server
const asked = `window.__state(${JSON.stringify(state)});`
  + ` openBranch(S.groups[0]);`
  + ` tellCheckout("staging", {project_name: "site", origin: "https://github.com/acme/site.git"}); "ok"`;

export default {
  settle: 1500,
  scenes: {
    list: `window.__state(${JSON.stringify(state)}); "ok"`,
    // The MicroVM's part put away: the rest of the project stays
    folded: `window.__state(${JSON.stringify(state)}); fold("machine:${vmSite}"); "ok"`,
    locate: asked,
    clone: asked.replace(/"ok"$/, 'apShow("sshclone"); "ok"'),
    drawer: {
      run: `window.__state(${JSON.stringify(state)});`
        + ` document.getElementById("app").classList.add("drawer"); "ok"`,
      sizes: [['phone', 390, 820]],
    },
  },
};
