/**
 * Throwing one file's change away, for tools/debug/shoot.mjs. How a scene file
 * is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/git-discard.mjs
 *
 * A right-click on a row of the changes column -- a held press on a phone --
 * opens a list with one entry in it, and that entry cannot be walked back. So
 * the entry is asked about before anything happens, and the question carries
 * the commands that will run, as the app built them.
 *
 * Four questions, because four rows mean four different things: a file changed
 * since the last commit, a file git has never seen (which goes off the disk),
 * a file already added to the next commit, and one added and never committed
 * (which goes off the disk too). The words have to be right for each one.
 */

const family = 'D:/work/shop/.git';

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0], group: 0, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);

const state = JSON.stringify({
  desk: 'shop', desk_id: 'shop', desks: ['shop'], desk_index: 0, active: 1,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    { name: 'shop', folder: 'D:/work/shop', color: '#4285f4', linked: false, family, branch: 'checkout-total',
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs: [tab(1, 'claude', { ai: 'claude' })],
  making: [],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
});

// git's own two letters: the staged side and the working-tree side
const row = (index, work, path, extra) => Object.assign({
  path, index, work, staged: index !== ' ' && index !== '?', unstaged: work !== ' ',
  conflict: false, tangled: false,
}, extra);

const rows = [
  row('M', ' ', 'src/checkout/total.ts'),
  row('A', ' ', 'src/checkout/coupon.ts'),
  row(' ', 'M', 'src/checkout/cart.ts'),
  row(' ', 'M', 'src/checkout/README.md'),
  row('?', '?', 'src/checkout/notes.scratch.md'),
];

const branch = {
  name: 'checkout-total', upstream: 'origin/checkout-total', ahead: 1, behind: 0,
  protected: false, detached: false,
};

// The column open on the changes, with the list filled in the way the app
// fills it
const setup = `
  window.__state(${JSON.stringify(state)});
  sidePanel = "git";
  setSideWidth(420);
  window.__git(${JSON.stringify({ act: 'branch', ok: true, data: branch })});
  window.__git(${JSON.stringify({ act: 'branches', ok: true, data: [{ name: 'checkout-total', current: true }, { name: 'main', current: false }] })});
  window.__git(${JSON.stringify({ act: 'status', ok: true, data: rows })});
  "ok"`;

// The row, found by the path written in it
const find = (path) => `[...document.querySelectorAll("#gitpanel .list .row")]
  .find(r => r.querySelector(".p") && r.querySelector(".p").textContent === ${JSON.stringify(path)})`;

// Pressing the right button on it, the way a pointer does
const openMenu = (path) => `new Promise(r => setTimeout(() => {
  const row = ${find(path)};
  if (!row) { r(Promise.reject(new Error("no row for ${path}"))); return; }
  const box = row.getBoundingClientRect();
  row.dispatchEvent(new MouseEvent("contextmenu", {bubbles:true, cancelable:true,
    clientX: Math.round(box.left + 60), clientY: Math.round(box.top + 10)}));
  r(document.querySelector(".fmenu") ? "ok" : Promise.reject(new Error("no menu opened")));
}, 400))`;

// ...and the phone's own way in: a finger held on the row. There is no right
// button there, and not every phone's browser turns a held press into one
const holdMenu = (path) => `new Promise(r => setTimeout(() => {
  const row = ${find(path)};
  if (!row) { r(Promise.reject(new Error("no row for ${path}"))); return; }
  const box = row.getBoundingClientRect();
  const at = {bubbles:true, cancelable:true, pointerType:"touch", isPrimary:true,
    clientX: Math.round(box.left + 60), clientY: Math.round(box.top + 10)};
  row.dispatchEvent(new PointerEvent("pointerdown", at));
  setTimeout(() => r(document.querySelector(".fmenu")
    ? "ok" : Promise.reject(new Error("holding the row opened nothing"))), 700);
}, 400))`;

// ...and choosing the one entry, then answering the way the app answers: the
// commands it would run, with nothing run
const ask = (path, said) => `${openMenu(path)}.then(() => {
  document.querySelector(".fmenu div").click();
  window.__git(${JSON.stringify({ act: 'discard', ok: true, data: { plan: true, said } })});
  return document.getElementById("sask").hidden
    ? Promise.reject(new Error("nothing was asked")) : "ok";
})`;

const lit = (p) => ':(literal)' + p;

export default {
  setup,
  settle: 1500,
  scenes: {
    // The list a row opens, on the one file in it that git has never seen
    menu: openMenu('src/checkout/notes.scratch.md'),
    // The same list, opened the way a finger opens it
    held: {
      run: holdMenu('src/checkout/notes.scratch.md'),
      served: 'remote',
      sizes: [['phone', 390, 820]],
    },
    // A file changed since the last commit: only the working side goes
    work: ask('src/checkout/cart.ts', ['git restore -- ' + lit('src/checkout/cart.ts')]),
    // One git has never seen: there is nothing to put back, so it goes
    fresh: ask('src/checkout/notes.scratch.md',
      ['git clean --force -d --quiet -- ' + lit('src/checkout/notes.scratch.md')]),
    // Already in the next commit: it goes back to the last commit, both sides
    staged: ask('src/checkout/total.ts',
      ['git restore --source=HEAD --staged --worktree -- ' + lit('src/checkout/total.ts')]),
    // Added to the next commit and never committed: out of the commit, off the disk
    added: ask('src/checkout/coupon.ts',
      ['git rm --force --quiet -- ' + lit('src/checkout/coupon.ts')]),
  },
};
