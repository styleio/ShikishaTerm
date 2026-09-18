/**
 * The name field of the worktree dialog, for tools/debug/shoot.mjs. How a
 * scene file is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/worktree-naming.mjs
 *
 * A branch and a folder are held to the letters every machine can hold, so
 * what somebody types is not always what gets made. These are the four things
 * that can happen to a typed name, each with the answer the app would send
 * back for it (uistate.rs, BranchPlan).
 */

const family = 'D:/work/site/.git';
const at = 'C:/Users/me/SHIKISHA-TERM/branches/site';

const state = JSON.stringify({
  desk: 'site', desk_id: 'site', desks: ['site'], desk_index: 0, active: 0,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    {
      name: 'site', folder: 'D:/work/site', color: '#4285f4', linked: false, family,
      branch: 'main', health: { as: 'fine' }, drift: { behind: 0, ahead: 0 },
    },
  ],
  tabs: [],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
});

// The answer the app sends while a name is being typed: what will be made,
// where it will go, and the command that does it
const answer = (typed, branch) => ({
  from: 'D:/work/site',
  asked: typed,
  branch,
  folder: at + '/' + branch,
  line: 'git -C D:/work/site worktree add --no-track -b ' + branch
    + ' ' + at + '/' + branch + ' origin/main',
  seq: 1,
  base: 'origin/main',
  bases: ['origin/main', 'main'],
  carry: [], carry_lines: [], lines: [],
  project: 'site', project_at: 'D:/work/site', project_name: 'site',
  hosts: [], host: '', setup_from: '', setup_unresolved: [],
});

// Opened on the project, with a name already in the box, and then the answer
// for that name pushed in the way the app pushes it
const typing = (typed, branch) => `
  window.__state(${JSON.stringify(state)});
  openBranch({folder: "D:/work/site"}, {name: ${JSON.stringify(typed)}});
  const s = JSON.parse(${JSON.stringify(state)});
  s.branch = ${JSON.stringify(answer(typed, branch))};
  s.branch.asked = ${JSON.stringify(typed)};
  window.__state(JSON.stringify(s));
  "ok"`;

export default {
  settle: 1500,
  scenes: {
    // All of it is letters a branch and a folder can both hold: nothing to say
    kept: typing('fix/crash-on-open', 'fix/crash-on-open'),
    // Some of it is: what is left stands, and the line under the box says so
    cut: typing('ログイン画面 login', 'login'),
    // None of it is: the app draws a name, and what was typed stays on the card
    drawn: typing('ログイン画面', 'polite-marmot'),
    // Nothing typed at all: the drawn name waits in the empty box
    empty: `
      window.__state(${JSON.stringify(state)});
      openBranch({folder: "D:/work/site"}, {});
      const s = JSON.parse(${JSON.stringify(state)});
      s.branch = ${JSON.stringify(answer('', 'mighty-gannet'))};
      window.__state(JSON.stringify(s));
      "ok"`,
  },
};
