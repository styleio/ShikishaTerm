/**
 * A new issue being made, for tools/debug/shoot.mjs. How a scene file is
 * written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/issue-making.mjs
 *
 * The Issue tab's form after its button was pressed: under the button, what
 * is being done, the seconds against the longest it can take, and a bar toward
 * that; the button and cancel grey. Then the button pressed again, which says
 * why above them. Then GitHub's answer in: the bar full at once, saying which
 * issue was made, until its page opens.
 */

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0], group: 0, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);

const state = JSON.stringify({
  desk: 'shop', desk_id: 'shop', desks: ['shop'], desk_index: 0, active: 2,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    { name: 'shop', folder: 'D:/work/shop', color: '#4285f4', linked: false, family: 'D:/work/shop/.git',
      branch: 'main', health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs: [tab(1, 'claude', { ai: 'claude' }), tab(2, 'Issues', { kind: 'issues' })],
  making: [],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
});

// The projects as the app answers them, with the longest a make can take
const setup = `
  window.__state(${JSON.stringify(state)});
  window.__issues(${JSON.stringify({
    act: 'projects', ok: true,
    projects: [{ name: 'shop', dir: 'D:/work/shop', at: 'D:/work/shop', repo: 'acme/shop', account: '' }],
    accounts: [], pc: [], gh: [], labels: {}, waits: { create: 50, create_pr: 50 },
  })});
  window.__issues(${JSON.stringify({ act: 'list', ok: true, seq: null, kind: 'issue', items: [], total: 0 })});
  window.__issues(${JSON.stringify({ act: 'options', ok: true, project: 'shop', data: { labels: ['bug', 'ui'], assignees: [] } })});
  I.view = "create";
  Object.assign(I.create, {project: "shop", title: "Checkout total ignores the coupon",
    body: "Applying SAVE10 shows the discount on the cart, but the order total is the full price.", labels: ["bug"]});
  "ok"`;

// Pressed twelve seconds ago, the answer not in yet
const making = `
  I.busy = "create"; I.busySince = Date.now() - 12000;
  issuesSig = ""; drawIssues(); "ok"`;

export default {
  setup,
  scenes: {
    'making': making,
    'making-pressed-again': making + `;
      document.querySelector("#issuespanel .makefoot button.go").click(); "ok"`,
    'made': `
      I.busy = "create"; I.busySince = Date.now() - 3000; drawIssues();
      window.__issues(${JSON.stringify({ act: 'create', ok: true, project: 'shop', data: { number: 41, url: 'https://github.com/acme/shop/issues/41' } })});
      "ok"`,
  },
};
