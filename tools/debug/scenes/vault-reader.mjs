/**
 * The conversation panel's "Every conversation", for tools/debug/shoot.mjs.
 *
 * The list the search of every conversation fills: a conversation of one of
 * this app's AI tabs (named by its tab), one on another machine, one found by
 * a note written on it and pinned, one whose words a phone sent -- and over a
 * conversation opened from it, the way back and Resume, with the places a
 * conversation whose worktree was removed can go instead. Answered the way the
 * app answers them, through `window.__convo` and `window.__vaultWhere`.
 */

const folder = 'D:/work/shop';
const hits = [
  {program: 'claude', id: 'aaaa', title: 'shop', cwd: folder, when: 1790000000, at: 120, thread: 'coder', thread_name: 'coder',
    snippet: '…The cart total is computed by the pricing module on every request…',
    said: {who: 'ai', when: null, head: 'The cart total'}},
  {program: 'claude', id: 'bbbb', title: 'shop', cwd: folder, when: 1789990000, at: 40,
    snippet: 'why is the pricing page slow on the phone?',
    said: {who: 'you', when: null, head: 'why is'}, from: {by: 'person', device: 'phone', via: 'input'}},
  {program: 'codex', id: 'cccc', title: 'vm: shop-fix-tax', cwd: '/home/user/shop-fix-tax', when: 1789900000, host: 'vm',
    snippet: 'Explain how the pricing rules apply tax to a discounted line…'},
  {program: 'claude', id: 'dddd', title: 'shop', cwd: folder, when: 1789800000, at: 0, pinned: true, noted: true,
    snippet: 'remember: pricing rounds half up'},
];

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, state: "DONE", state_label: "Done", profile: "Claude Code",
  locked: false, depth: 0, activity: [0, 1, 3, 2, 0, 0, 1, 0, 0, 0], group: 0, kind: "pty",
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: true, key: "tab:" + index,
}, extra);

const state = JSON.stringify({
  desk: "work", desk_id: "work", desks: ["work"], desk_index: 0, active: 1,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [{ name: "shop", folder, color: "#5b7cff", linked: false, family: folder + "/.git", branch: "main",
    health: { as: "fine" }, drift: { behind: 0, ahead: 0 } }],
  tabs: [tab(1, "coder", { ai: "claude" })],
  jobs: [],
  vault: { query: "pricing", hits, capped: false },
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: "", progress: 0, awaiting_human: false },
  auto_enabled: true, build: "", help_rows: [], ais: [],
});

const setup = `
  window.__state(${JSON.stringify(state)});
  setSideWidth(420);
  window.__openVault();
  cvUi.q.value = "pricing"; CV.q = "pricing";
  allFindNow();
  window.__convo({panel:"vault", act:"find", ok:true, req:cvAllSeq.all, vault:S.vault});
  delete cvUi.list.dataset.all;
  drawConvo();
  "ok"`;

// A conversation opened from the list, its folder gone, the places offered
const gone = `
  cvAll = false; cvFromAll = true;
  CV.past = {program: "codex", id: "gone", host: ""}; CV.loading = false; CV.rows = []; CV.rev++;
  cvWhereReq = convoRequest(CV.seq, "where");
  window.__vaultWhere({req: cvWhereReq, ok: true, program: "codex", id: "gone", folder: "D:/work/shop-fix-tax",
    exists: false, branch: "fix/tax", homes: [{dir: "${folder}", local: true}]});
  drawConvo();
  document.querySelector("#convoHead .hgo").click();
  "ok"`;

export default {
  setup,
  scenes: {
    list: 'drawConvo(); "ok"',
    gone: { run: gone, looks: ['dark'] },
  },
  settle: 1200,
};
