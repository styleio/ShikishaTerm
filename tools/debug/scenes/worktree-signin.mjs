/**
 * The sign-in part of the worktree dialog, for tools/debug/shoot.mjs. How a
 * scene file is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/worktree-signin.mjs
 *
 * A worktree made on a MicroVM is a copy of the checkout's machine, so the
 * dialog says two things about that machine before anything is made: what it
 * signs in to the git server as, and whether the AI there is signed in. These
 * are the shapes those two take (uistate.rs, BranchPlan's sign_in and
 * ai_sign_in), each with the answer the app would send back.
 */

const family = 'D:/work/site/.git';
const checkout = '/home/user/site';
const at = '/home/user/branches/site';

const state = JSON.stringify({
  desk: 'site', desk_id: 'site', desks: ['site'], desk_index: 0, active: 0,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    {
      name: 'site', folder: checkout, color: '#4285f4', linked: false, family,
      branch: 'main', health: { as: 'fine' }, drift: { behind: 0, ahead: 0 },
    },
  ],
  // The checkout's own AI tab, which the "open" button goes to
  tabs: [{ index: 1, title: 'claude', id: 'claude', group: 0, ai: 'claude', kind: 'pty', state: 'WAIT' }],
  hosts: [{ name: 'check', kind: 'microvm' }],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
});

const answer = (signIn, aiSignIn) => ({
  from: checkout,
  asked: 'fix-login',
  branch: 'fix-login',
  folder: at + '/fix-login',
  line: 'git -C ' + checkout + ' worktree add --no-track -b fix-login ' + at + '/fix-login origin/main',
  seq: 1,
  base: 'origin/main',
  bases: ['origin/main', 'main'],
  carry: [], carry_lines: [], lines: [],
  project: 'site', project_at: checkout, project_name: 'site',
  hosts: [{ name: 'check', kind: 'microvm', at: checkout }],
  host: 'check',
  sign_in: signIn,
  ai_sign_in: aiSignIn,
  setup_from: '', setup_unresolved: [],
});

const open = (signIn, aiSignIn) => `
  window.__state(${JSON.stringify(state)});
  openBranch({folder: ${JSON.stringify(checkout)}}, {name: "fix-login"});
  const s = JSON.parse(${JSON.stringify(state)});
  s.branch = ${JSON.stringify(answer(signIn, aiSignIn))};
  window.__state(JSON.stringify(s));
  "ok"`;

const fine = { account: 'check', kind: 'fine' };
const ai = (state) => ({ state, name: 'Claude Code', checkout, ai: 'claude', on: 'microvm' });

export default {
  settle: 1500,
  scenes: {
    // The case that was reported: a good token, and the AI not signed in yet
    'ai-not-signed-in': open(fine, ai('no')),
    // Both fine: two quiet lines
    'all-fine': open(fine, ai('yes')),
    // Signed in, first run not finished
    'ai-finishing': open(fine, ai('finishing')),
    // A token that reaches too far, with what to use instead
    'broad-token': open({ account: 'check', kind: 'classic' }, ai('yes')),
    // Still being asked
    asking: open(null, ai('asking')),
  },
};
