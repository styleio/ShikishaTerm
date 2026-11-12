/**
 * A GitHub Project on the Issue tab, for tools/debug/shoot.mjs. How a scene
 * file is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/github-project.mjs
 *
 * The third kind beside issues and pull requests: a board view, its cards in
 * columns by Status with the one that has none last; the same board's table
 * view, grouped and with its fields to change in place; a card moved twice in
 * a row, shown in its new column at once and both changes queued for GitHub
 * in the order they were made; the list of GitHub
 * Projects to choose from; and the account's token refused, with the way to
 * its settings. Answers are put in the way the app sends them, so nothing
 * here waits on GitHub.
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

const ADDRESS = 'https://github.com/orgs/acme/projects/3';
const fields = [
  { id: 'F_title', name: 'Title', type: 'TITLE' },
  { id: 'F_assignees', name: 'Assignees', type: 'ASSIGNEES' },
  { id: 'F_status', name: 'Status', type: 'SINGLE_SELECT', options: [
    { id: 's_todo', name: 'Todo', color: 'GRAY' }, { id: 's_doing', name: 'In progress', color: 'YELLOW' },
    { id: 's_review', name: 'In review', color: 'BLUE' }, { id: 's_done', name: 'Done', color: 'GREEN' }] },
  { id: 'F_pri', name: 'Priority', type: 'SINGLE_SELECT', options: [
    { id: 'p_high', name: 'High' }, { id: 'p_mid', name: 'Medium' }, { id: 'p_low', name: 'Low' }] },
  { id: 'F_due', name: 'Due', type: 'DATE' },
  { id: 'F_size', name: 'Estimate', type: 'NUMBER' },
  { id: 'F_labels', name: 'Labels', type: 'LABELS' },
];
const card = (id, number, title, status, extra) => Object.assign({
  id, kind: 'issue', number, title, url: 'https://github.com/acme/shop/issues/' + number, state: 'open',
  repo: 'acme/shop', assignees: [], labels: [], updated: '2026-11-10T09:00:00Z',
  values: status ? { F_status: { option: status, name: fields[2].options.find((o) => o.id === status).name } } : {},
}, extra);
const cards = [
  card('c1', 41, 'Checkout total ignores the coupon', 's_doing', { assignees: ['kim'], labels: [{ name: 'bug', color: 'd73a4a' }],
    values: { F_status: { option: 's_doing', name: 'In progress' }, F_pri: { option: 'p_high', name: 'High' }, F_due: { date: '2026-11-20' }, F_size: { number: 3 } } }),
  card('c2', 44, 'Show the delivery date on the order page', 's_todo', { labels: [{ name: 'ui', color: '0e8a16' }],
    values: { F_status: { option: 's_todo', name: 'Todo' }, F_pri: { option: 'p_mid', name: 'Medium' } } }),
  card('c3', 39, 'Speed up the product search', 's_review', { kind: 'pr', state: 'open', url: 'https://github.com/acme/shop/pull/39',
    assignees: ['lee'], values: { F_status: { option: 's_review', name: 'In review' }, F_pri: { option: 'p_high', name: 'High' }, F_size: { number: 5 } } }),
  card('c4', 37, 'Gift wrapping option', 's_done', { state: 'closed', assignees: ['kim'] }),
  card('c5', 0, 'Ask support which refund cases come up most', 's_todo', { kind: 'draft', number: null, url: null, repo: null }),
  card('c6', 12, 'Translate the emails into German', null, { repo: 'acme/mailer', url: 'https://github.com/acme/mailer/issues/12' }),
];
const views = [
  { number: 1, name: 'Board', layout: 'board', filter: '', fields: [], group: null, lanes: 'F_status', sort: [] },
  { number: 3, name: 'By priority', layout: 'board', filter: '', fields: [], group: 'F_pri', lanes: 'F_status', sort: [] },
  { number: 2, name: 'Plan', layout: 'table', filter: 'is:open', fields: ['F_title', 'F_assignees', 'F_status', 'F_pri', 'F_due', 'F_size'],
    group: 'F_pri', lanes: null, sort: [{ field: 'F_due', desc: false }] },
];
const answer = (view) => ({
  act: 'gh_project', ok: true, project: 'shop', data: {
    id: 'PVT_1', number: 3, title: 'Shop roadmap', url: ADDRESS, closed: false, owner: 'acme', org: true,
    fields, views, view, filter: views.find((w) => w.number === view).filter, columns: 'F_status',
    cards: view === 2 ? cards.filter((c) => c.state === 'open') : cards, ref: ADDRESS,
  },
});

// The projects as the app answers them, one with this GitHub Project chosen
const setup = `
  window.__state(${JSON.stringify(state)});
  window.__issues(${JSON.stringify({
    act: 'projects', ok: true,
    projects: [{ name: 'shop', dir: 'D:/work/shop', at: 'D:/work/shop', repo: 'acme/shop', account: '', gh_project: ADDRESS, kept: true }],
    accounts: [], pc: [], gh: [], labels: {}, waits: { create: 50, create_pr: 50 },
  })});
  I.kind = "ghp"; I.list = null;
  "ok"`;

// An answer put in as the newest one asked for
const put = (d) => `window.__issues(Object.assign(${JSON.stringify(d)}, {seq: I.want.gh_project})); "ok"`;

export default {
  setup,
  scenes: {
    'board': `ghpRead(); ` + put(answer(1)),
    'moved': `ghpRead(); ` + put(answer(1)).replace(/; "ok"$/, '') + `;
      const d = I.gh.data, f = d.fields.find(x => x.id === "F_status"), c = d.cards.find(x => x.id === "c2");
      ghpSet(c, f, {option: "s_review"}); ghpSet(c, f, {option: "s_doing"});
      if (I.gh.sending.length !== 2 || I.gh.sending[1].args.value.option !== "s_doing") throw new Error("not queued in order");
      "ok"`,
    'table': `I.gh.view = 2; ghpRead(); ` + put(answer(2)),
    'lanes': `I.gh.view = 3; ghpRead(); ` + put(answer(3)),
    'choose': `I.gh.picking = true; issuesSig = ""; drawIssues();
      window.__issues(${JSON.stringify({ act: 'gh_projects', ok: true, project: 'shop', data: { hidden: 1, projects: [
        { id: 'PVT_1', number: 3, title: 'Shop roadmap', owner: 'acme', org: true, url: ADDRESS, closed: false, cards: 6, from: 'repository' },
        { id: 'PVT_2', number: 1, title: 'Support requests', owner: 'acme', org: true, url: 'https://github.com/orgs/acme/projects/1', closed: false, cards: 48, from: 'owner' },
        { id: 'PVT_9', number: 2, title: 'Reading list', owner: 'kim', org: false, url: 'https://github.com/users/kim/projects/2', closed: true, cards: 12, from: 'mine' }] } })}); "ok"`,
    'denied': {
      run: `ghpRead(); window.__issues(Object.assign({act: "gh_project", ok: false, project: "shop", settings: true,
        error: T["err.ghproject.denied"]}, {seq: I.want.gh_project})); "ok"`,
      sizes: [['wide', 1280, 860]],
    },
  },
};
