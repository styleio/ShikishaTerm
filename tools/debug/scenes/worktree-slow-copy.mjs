/**
 * The worktree dialog when what it would copy is large, for
 * tools/debug/shoot.mjs. How a scene file is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/worktree-slow-copy.mjs
 *
 * What the new folder inherits is counted on a thread and arrives after the
 * dialog has opened (BranchPlan.carry_sizes). A large copy is said above the
 * fold, before the button is pressed; under the fold each row says what it
 * holds, and the large ones are marked.
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

const line = (pattern) => ({ source: '.gitignore', pattern });
const carry = [
  { name: 'node_modules', folder: true, how: 'copy', line: line('node_modules/') },
  { name: 'target', folder: true, how: 'copy', line: line('/target') },
  { name: '.env', folder: false, how: 'copy', line: line('.env') },
  { name: 'vendor', folder: true, how: 'link', line: line('vendor/') },
];
const GB = 1024 * 1024 * 1024;
const sizes = [
  { path: 'node_modules', bytes: Math.round(0.6 * GB), files: 50000, more: true },
  { path: 'target', bytes: Math.round(4.2 * GB), files: 18342, more: false },
  { path: '.env', bytes: 412, files: 1, more: false },
  { path: 'vendor', bytes: Math.round(0.3 * GB), files: 9120, more: false },
];

const answer = (counted) => ({
  from: 'D:/work/site',
  asked: '',
  branch: 'mighty-gannet',
  folder: at + '/mighty-gannet',
  line: 'git -C D:/work/site worktree add --no-track -b mighty-gannet ' + at + '/mighty-gannet origin/main',
  seq: 1,
  base: 'origin/main',
  bases: ['origin/main', 'main'],
  carry,
  carry_lines: [
    { source: '.gitignore', pattern: 'node_modules/', how: 'copy', count: 1, folders: true },
    { source: '.gitignore', pattern: '/target', how: 'copy', count: 1, folders: true },
    { source: '.gitignore', pattern: '.env', how: 'copy', count: 1, folders: false },
    { source: '.gitignore', pattern: 'vendor/', how: 'link', count: 1, folders: true },
  ],
  carry_sizes: counted ? sizes : null,
  large_bytes: GB, large_files: 50000,
  lines: [],
  project: 'site', project_at: 'D:/work/site', project_name: 'site',
  hosts: [], host: '', setup_from: '', setup_unresolved: [],
});

const opened = (counted, then) => `
  window.__state(${JSON.stringify(state)});
  openBranch({folder: "D:/work/site"}, {});
  const s = JSON.parse(${JSON.stringify(state)});
  s.branch = ${JSON.stringify(answer(counted))};
  window.__state(JSON.stringify(s));
  ${then || ''}
  "ok"`;

export default {
  settle: 1500,
  scenes: {
    // Still being counted: nothing said yet
    counting: opened(false),
    // Counted: the warning above the fold
    warned: opened(true),
    // Its button pressed: the fold open on the rows that cost it
    shown: opened(true, `document.querySelector("#branch .bslow .bwarn button").click();`),
  },
};
