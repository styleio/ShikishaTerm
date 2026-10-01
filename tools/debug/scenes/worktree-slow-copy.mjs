/**
 * The worktree dialog when what it would copy is large, for
 * tools/debug/shoot.mjs. How a scene file is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/worktree-slow-copy.mjs
 *
 * What the new folder inherits is counted on a thread and arrives after the
 * dialog has opened (BranchPlan.carry_sizes). A large copy is said above the
 * fold, before the button is pressed; under the fold each row says what it
 * holds, the large ones marked, and a folder opens to what is inside it
 * (BranchPlan.looks), where a place can be given its own rule.
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

const GB = 1024 * 1024 * 1024;
const MB = 1024 * 1024;
const line = (pattern) => ({ source: '.gitignore', pattern });
const size = (path, bytes, files, more = false) => ({ path, bytes: Math.round(bytes), files, more });

// Before any rule: .claude is copied whole, the other agents' worktrees in it
const before = {
  carry: [
    { name: '.claude', folder: true, how: 'copy', line: line('.claude/') },
    { name: 'node_modules', folder: true, how: 'copy', line: line('node_modules/') },
    { name: '.env', folder: false, how: 'copy', line: line('.env') },
    { name: 'vendor', folder: true, how: 'link', line: line('vendor/') },
  ],
  sizes: [
    size('.claude', 147.7 * GB, 50000, true),
    size('node_modules', 0.6 * GB, 41203),
    size('.env', 412, 1),
    size('vendor', 0.3 * GB, 9120),
  ],
  looks: [{
    path: '.claude',
    items: [
      { size: size('.claude/worktrees', 147.6 * GB, 50000, true), folder: true, how: 'copy' },
      { size: size('.claude/hooks', 1.2 * MB, 38), folder: true, how: 'copy' },
      { size: size('.claude/skills', 0.4 * MB, 21), folder: true, how: 'copy' },
      { size: size('.claude/RULES.md', 41 * 1024, 1), folder: false, how: 'copy' },
      { size: size('.claude/settings.json', 3 * 1024, 1), folder: false, how: 'copy' },
    ],
  }],
};

// After "Leave out" was chosen for .claude/worktrees: the project's rule
const after = {
  carry: [
    { name: '.claude', folder: true, how: 'copy', line: line('.claude/'), ruled_inside: true },
    ...before.carry.slice(1),
  ],
  sizes: [size('.claude', 1.7 * MB, 61), ...before.sizes.slice(1)],
  looks: [{
    path: '.claude',
    items: [
      { size: size('.claude/worktrees', 147.6 * GB, 50000, true), folder: true, how: 'skip', by: '.claude/worktrees/' },
      ...before.looks[0].items.slice(1),
    ],
  }],
};

const answer = (shown, counted) => ({
  from: 'D:/work/site',
  asked: '',
  branch: 'mighty-gannet',
  folder: at + '/mighty-gannet',
  line: 'git -C D:/work/site worktree add --no-track -b mighty-gannet ' + at + '/mighty-gannet origin/main',
  seq: 1,
  base: 'origin/main',
  bases: ['origin/main', 'main'],
  carry: shown.carry,
  carry_lines: [
    { source: '.gitignore', pattern: '.claude/', how: 'copy', count: 1, folders: true },
    { source: '.gitignore', pattern: 'node_modules/', how: 'copy', count: 1, folders: true },
    { source: '.gitignore', pattern: '.env', how: 'copy', count: 1, folders: false },
    { source: '.gitignore', pattern: 'vendor/', how: 'link', count: 1, folders: true },
  ],
  carry_sizes: counted ? shown.sizes : null,
  looks: counted ? shown.looks : [],
  large_bytes: GB, large_files: 50000,
  lines: [],
  project: 'site', project_at: 'D:/work/site', project_name: 'site',
  hosts: [], host: '', setup_from: '', setup_unresolved: [],
});

const opened = (shown, counted, then) => `
  window.__state(${JSON.stringify(state)});
  openBranch({folder: "D:/work/site"}, {});
  const s = JSON.parse(${JSON.stringify(state)});
  s.branch = ${JSON.stringify(answer(shown, counted))};
  window.__state(JSON.stringify(s));
  ${then || ''}
  window.__state(JSON.stringify(s));
  "ok"`;

const see = `document.querySelector("#branch .bslow .bwarn button").click();`;

export default {
  settle: 1500,
  scenes: {
    // Still being counted: nothing said yet
    counting: opened(before, false),
    // Counted: the warning above the fold
    warned: opened(before, true),
    // Its button pressed: the fold open on the row that costs it, opened
    shown: opened(before, true, see),
    // .claude/worktrees left out by the project's rule: no warning, and the
    // row says which rule decided it
    ruled: opened(after, true, `showMore(document.getElementById("branch"), true);
      document.querySelector('#branch .bcarry .crow button.bcopen').click();
      document.querySelector('#branch .bcarry').scrollIntoView({block: 'start'});`),
    // .claude linked while a rule names a place inside it
    linked: opened({ ...after, carry: [{ ...after.carry[0], how: 'link' }, ...after.carry.slice(1)] }, true,
      `showMore(document.getElementById("branch"), true);
      document.querySelector('#branch .bcarry').scrollIntoView({block: 'start'});`),
  },
};
