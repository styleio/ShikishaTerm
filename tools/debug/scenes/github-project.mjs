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
 * its settings. Then what is done to the board from here: a table of every
 * kind of field with a cell's list open, cards chosen to be changed together,
 * issues found for what is typed where a card is added, the board's archive,
 * a board with more cards than were read, a view sliced on this screen, the
 * pages of its views, its fields, its charts and its own settings, a new
 * board being made, and an issue of a repository the desk has no project
 * for, opened on this tab with its sub-issues. Answers are put in the way the
 * app sends them, so nothing here waits on GitHub.
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
  { id: 'F_start', name: 'Start', type: 'DATE' },
  { id: 'F_due', name: 'Due', type: 'DATE' },
  { id: 'F_size', name: 'Estimate', type: 'NUMBER' },
  { id: 'F_labels', name: 'Labels', type: 'LABELS' },
  { id: 'F_tags', name: 'Area', type: 'MULTI_SELECT', options: [
    { id: 't_web', name: 'Web', color: 'BLUE' }, { id: 't_app', name: 'App', color: 'PURPLE' }, { id: 't_mail', name: 'Email', color: 'ORANGE' }] },
  { id: 'F_ms', name: 'Milestone', type: 'MILESTONE' },
  { id: 'F_type', name: 'Type', type: 'ISSUE_TYPE' },
  { id: 'F_parent', name: 'Parent issue', type: 'PARENT_ISSUE' },
  { id: 'F_sub', name: 'Sub-issues progress', type: 'SUB_ISSUES_PROGRESS' },
  { id: 'F_prs', name: 'Linked pull requests', type: 'LINKED_PULL_REQUESTS' },
  { id: 'F_rev', name: 'Reviewers', type: 'REVIEWERS' },
  { id: 'F_sprint', name: 'Sprint', type: 'ITERATION', days: 14, iterations: [
    { id: 'i1', title: 'Sprint 1', start: '2026-10-26', days: 14, done: true }, { id: 'i2', title: 'Sprint 2', start: '2026-11-09', days: 14, done: false },
    { id: 'i3', title: 'Sprint 3', start: '2026-11-23', days: 14, done: false }] },
];
const card = (id, number, title, status, extra) => Object.assign({
  id, kind: 'issue', number, title, url: 'https://github.com/acme/shop/issues/' + number, state: 'open',
  repo: 'acme/shop', assignees: [], labels: [], updated: '2026-11-10T09:00:00Z', created: '2026-09-' + String(10 + (number % 19)).padStart(2, '0') + 'T09:00:00Z',
  values: status ? { F_status: { option: status, name: fields[2].options.find((o) => o.id === status).name } } : {},
}, extra);
const cards = [
  card('c1', 41, 'Checkout total ignores the coupon', 's_doing', { assignees: ['kim'], labels: [{ name: 'bug', color: 'd73a4a' }],
    milestone: 'Winter sale', type: 'Bug', sub: { total: 4, done: 1, percent: 25 },
    prs: [{ number: 39, url: 'https://github.com/acme/shop/pull/39', repo: 'acme/shop', state: 'open' }],
    values: { F_status: { option: 's_doing', name: 'In progress' }, F_pri: { option: 'p_high', name: 'High' }, F_start: { date: '2026-11-02' }, F_due: { date: '2026-11-20' }, F_size: { number: 3 },
      F_tags: { options: [{ id: 't_web', name: 'Web' }, { id: 't_app', name: 'App' }] },
      F_sprint: { iteration: 'i2', title: 'Sprint 2', start: '2026-11-09', days: 14 } } }),
  card('c2', 44, 'Show the delivery date on the order page', 's_todo', { labels: [{ name: 'ui', color: '0e8a16' }],
    parent: { number: 41, title: 'Checkout total ignores the coupon', url: 'https://github.com/acme/shop/issues/41', repo: 'acme/shop' },
    values: { F_status: { option: 's_todo', name: 'Todo' }, F_pri: { option: 'p_mid', name: 'Medium' }, F_start: { date: '2026-11-24' }, F_due: { date: '2026-12-18' } } }),
  card('c3', 39, 'Speed up the product search', 's_review', { kind: 'pr', state: 'open', url: 'https://github.com/acme/shop/pull/39',
    assignees: ['lee'], values: { F_status: { option: 's_review', name: 'In review' }, F_pri: { option: 'p_high', name: 'High' }, F_start: { date: '2026-10-19' }, F_due: { date: '2026-11-13' }, F_size: { number: 5 } },
    reviewers: ['kim', { team: true }] }),
  card('c4', 37, 'Gift wrapping option', 's_done', { state: 'closed', assignees: ['kim'], closed_at: '2026-10-30T09:00:00Z' }),
  card('c5', 0, 'Ask support which refund cases come up most', 's_todo', { kind: 'draft', number: null, url: null, repo: null,
    draft: 'DI_5', body: 'Sort last month\x27s refund tickets by reason, then ask support which they see most.' }),
  card('c6', 12, 'Translate the emails into German', null, { repo: 'acme/mailer', url: 'https://github.com/acme/mailer/issues/12' }),
];
const views = [
  { id: 'V_1', number: 1, name: 'Board', layout: 'board', filter: '', fields: [], group: null, lanes: 'F_status', sort: [] },
  { id: 'V_4', number: 4, name: 'Timeline', layout: 'roadmap', filter: '', fields: [], group: 'F_pri', lanes: null, sort: [] },
  { id: 'V_3', number: 3, name: 'By priority', layout: 'board', filter: '', fields: [], group: 'F_pri', lanes: 'F_status', sort: [] },
  { id: 'V_2', number: 2, name: 'Plan', layout: 'table', filter: 'is:open', fields: ['F_title', 'F_assignees', 'F_status', 'F_pri', 'F_due', 'F_size'],
    group: 'F_pri', lanes: null, sort: [{ field: 'F_due', desc: false }] },
  // Every kind of field a table shows, the ones GitHub fills in from the issue among them
  { id: 'V_5', number: 5, name: 'Everything', layout: 'table', filter: '', fields: ['F_title', 'F_assignees', 'F_labels', 'F_tags', 'F_ms', 'F_type',
    'F_parent', 'F_sub', 'F_prs', 'F_rev', 'F_sprint'], group: null, lanes: null, sort: [] },
];
const answer = (view, more) => ({
  act: 'gh_project', ok: true, project: 'shop', data: Object.assign({
    id: 'PVT_1', number: 3, title: 'Shop roadmap', url: ADDRESS, closed: false, public: false, description: '', can_update: true, owner: 'acme', org: true,
    fields, views, view, filter: views.find((w) => w.number === view).filter, columns: 'F_status',
    cards: view === 2 ? cards.filter((c) => c.state === 'open') : cards, ref: ADDRESS, total: cards.length, more: false, archived: false,
  }, more),
});
// What a card's repository offers its issues
const offers = { labels: ['bug', 'ui', 'docs', 'good first issue'], assignees: ['kim', 'lee', 'park'],
  milestones: [{ number: 1, title: 'Winter sale' }, { number: 2, title: 'Spring' }], types: ['Bug', 'Feature', 'Task'], templates: { items: [], blank: true, problems: [] } };
// What the board says of itself
const about = { id: 'PVT_1', number: 3, title: 'Shop roadmap', url: ADDRESS, org: true, owner: 'acme', closed: false, public: false, template: false,
  can_update: true, description: 'What the shop team ships this quarter', readme: 'Cards move left to right. Ask in #shop before adding a column.',
  repos: ['acme/shop', 'acme/mailer'],
  updates: [{ id: 'SU_2', status: 'at_risk', body: 'Search is a week behind: the index rebuild took longer than planned.', start: '2026-10-01', target: '2026-12-18', created: '2026-11-10T09:00:00Z', author: 'kim' },
    { id: 'SU_1', status: 'on_track', body: 'Checkout fixes are in review.', start: '2026-10-01', target: '2026-12-18', created: '2026-10-27T09:00:00Z', author: 'lee' }],
  workflows: [{ id: 'W_1', name: 'Auto-add to project', number: 1, enabled: false }, { id: 'W_2', name: 'Item closed', number: 2, enabled: true }] };
// An issue of a repository the desk has no project for
const far = { number: 12, kind: 'issue', title: 'Translate the emails into German', state: 'open', reason: null, draft: false, author: 'park',
  labels: ['docs'], assignees: ['lee'], milestone: 'Spring', type: 'Task', comments: [], events: [], created: '2026-11-01T09:00:00Z', updated: '2026-11-10T09:00:00Z',
  url: 'https://github.com/acme/mailer/issues/12', body: 'Order, shipping and refund emails. The wording is in `templates/de`.', node: 'I_12',
  parent: { id: 'I_3', number: 3, title: 'Sell in Germany', state: 'open', url: 'https://github.com/acme/mailer/issues/3', repo: 'acme/mailer' },
  subs: [{ id: 'I_13', number: 13, title: 'Order confirmation', state: 'closed', url: 'https://github.com/acme/mailer/issues/13', repo: 'acme/mailer' },
    { id: 'I_14', number: 14, title: 'Shipping notice', state: 'open', url: 'https://github.com/acme/mailer/issues/14', repo: 'acme/mailer' },
    { id: 'I_52', number: 52, title: 'Show the German price format', state: 'open', url: 'https://github.com/acme/shop/issues/52', repo: 'acme/shop' }] };

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
    'roadmap': `I.gh.view = 4; ghpRead(); ` + put(answer(4)),
    // A draft opened to be written, its fields under it
    'draft': `ghpRead(); ` + put(answer(1)).replace(/; "ok"$/, '') + `; ghpEdit(I.gh.data.cards.find(c => c.id === "c5")); "ok"`,
    // A card being typed into the Todo column's box
    'adding': `ghpRead(); ` + put(answer(1)).replace(/; "ok"$/, '') + `; Object.assign(I.gh, {adding: "board  s_todo", addText: "Wrap gifts in recycled paper"}); issuesSig = ""; drawIssues(); "ok"`,
    // A card about to be taken off, waiting for a yes; and one just archived
    'remove': `ghpRead(); ` + put(answer(1)).replace(/; "ok"$/, '') + `; I.gh.confirm = {card: "c5", title: "Ask support which refund cases come up most"}; drawIssues(); "ok"`,
    'archived': `I.gh.view = 2; ghpRead(); ` + put(answer(2)).replace(/; "ok"$/, '') + `; I.gh.undo = {card: "c4", title: "Gift wrapping option", was: null}; drawIssues(); "ok"`,
    'choose': `I.gh.picking = true; issuesSig = ""; drawIssues();
      window.__issues(${JSON.stringify({ act: 'gh_projects', ok: true, project: 'shop', data: { hidden: 1, projects: [
        { id: 'PVT_1', number: 3, title: 'Shop roadmap', owner: 'acme', org: true, url: ADDRESS, closed: false, cards: 6, from: 'repository' },
        { id: 'PVT_2', number: 1, title: 'Support requests', owner: 'acme', org: true, url: 'https://github.com/orgs/acme/projects/1', closed: false, cards: 48, from: 'owner' },
        { id: 'PVT_9', number: 2, title: 'Reading list', owner: 'kim', org: false, url: 'https://github.com/users/kim/projects/2', closed: true, cards: 12, from: 'mine' }] } })}); "ok"`,
    // Every kind of field in a table, and the list a cell opens: the people
    // the card's repository offers, the ones on it ticked
    'cells': `I.gh.view = 5; ghpRead(); ` + put(answer(5)),
    'cell-pick': `I.gh.view = 5; ghpRead(); ` + put(answer(5)).replace(/; "ok"$/, '') + `;
      I.options["@acme/shop"] = ${JSON.stringify(offers)};
      ghpPickAgain("c1", "F_assignees");
      if (!document.querySelector(".fmenu .grouprow")) throw new Error("the cell's list did not open");
      "ok"`,
    // Cards chosen to be changed together, a date being typed for them
    'selecting': `ghpRead(); ` + put(answer(1)).replace(/; "ok"$/, '') + `;
      Object.assign(I.gh, {selecting: true, sel: {c1: true, c2: true}, bulkField: "F_due", bulkText: "2026-12-01"}); I.gh.rev++; drawIssues(); "ok"`,
    // Issues found for what is typed where a card is added
    'found': `ghpRead(); ` + put(answer(1)).replace(/; "ok"$/, '') + `;
      Object.assign(I.gh, {adding: "board  s_todo", addText: "delivery", findWant: "ghp-f1",
        found: {text: "delivery", waiting: 0, items: [
          {number: 44, title: "Show the delivery date on the order page", repo: "acme/shop", kind: "issue", state: "open"},
          {number: 51, title: "Delivery estimate is a day off for islands", repo: "acme/shop", kind: "issue", state: "open"},
          {number: 48, title: "Cache the delivery calendar", repo: "acme/shop", kind: "pr", state: "merged"}]}});
      issuesSig = ""; drawIssues(); "ok"`,
    // The board's archive, and a board with more cards than were read
    'archive': `I.gh.archive = true; ghpRead(); ` + put(answer(1, { archived: true, cards: cards.slice(3, 5) })),
    'more-cards': `ghpRead(); ` + put(answer(1, { total: 2400, more: true })),
    // A view sliced on this screen by Priority, Medium looked at
    'sliced': `try { localStorage.setItem("ghp.shape", JSON.stringify({${JSON.stringify(ADDRESS + ' 1')}: {slice: "F_pri"}})); } catch (e) {}
      ghpRead(); ` + put(answer(1)).replace(/; "ok"$/, '') + `; I.gh.slice = {key: "p_high"}; I.gh.rev++; drawIssues();
      try { localStorage.removeItem("ghp.shape"); } catch (e) {}
      "ok"`,
    // The pages about the board itself
    'views': `ghpRead(); ` + put(answer(1)).replace(/; "ok"$/, '') + `; I.gh.panel = "views"; issuesSig = ""; drawIssues(); "ok"`,
    'fields': `ghpRead(); ` + put(answer(1)).replace(/; "ok"$/, '') + `; I.gh.panel = "fields"; ghpFieldOpen(I.gh.data.fields.find(f => f.id === "F_pri")); "ok"`,
    'charts': `ghpRead(); ` + put(answer(1)).replace(/; "ok"$/, '') + `; I.gh.panel = "charts";
      try { localStorage.setItem("ghp.chart", JSON.stringify({${JSON.stringify(ADDRESS)}: {kind: "bars", x: "F_status", by: "F_pri"}})); } catch (e) {}
      issuesSig = ""; drawIssues(); "ok"`,
    'charts-time': `ghpRead(); ` + put(answer(1)).replace(/; "ok"$/, '') + `; I.gh.panel = "charts";
      try { localStorage.setItem("ghp.chart", JSON.stringify({${JSON.stringify(ADDRESS)}: {kind: "time"}})); } catch (e) {}
      issuesSig = ""; drawIssues();
      try { localStorage.removeItem("ghp.chart"); } catch (e) {}
      "ok"`,
    'about': `ghpRead(); ` + put(answer(1)).replace(/; "ok"$/, '') + `; I.gh.panel = "about"; I.gh.about = ${JSON.stringify(about)}; issuesSig = ""; drawIssues(); "ok"`,
    // A new board being made, from the list of them
    'new': `I.gh.picking = true; I.gh.newp = {title: "Spring campaign", owner: "acme", link: true}; issuesSig = ""; drawIssues();
      window.__issues(${JSON.stringify({ act: 'gh_projects', ok: true, project: 'shop', data: { hidden: 0, projects: [
        { id: 'PVT_1', number: 3, title: 'Shop roadmap', owner: 'acme', org: true, url: ADDRESS, closed: false, cards: 6, from: 'repository' }] } })}); "ok"`,
    // An issue of a repository the desk has no project for, opened from its
    // card: its facts with the labels being changed, and its sub-issues
    'far-issue': `ghpRead(); ` + put(answer(1)).replace(/; "ok"$/, '') + `;
      I.options["@acme/mailer"] = ${JSON.stringify(offers)};
      ghpOpen(I.gh.data.cards.find(c => c.id === "c6"));
      window.__issues({act: "detail", ok: true, project: "shop", at_repo: "acme/mailer", kind: "issue", seq: I.want.detail, number: 12, data: ${JSON.stringify(far)}});
      if (!I.detail || I.detail.at_repo !== "acme/mailer") throw new Error("the card's issue did not open on this tab");
      // Two labels pressed one after the other: each said as the one name it
      // adds, and the second held until the first is answered
      issueListEdit(I.detail, "labels", ["docs", "bug"]); issueListEdit(I.detail, "labels", ["docs", "bug", "ui"]);
      if (I.edits.length !== 2 || JSON.stringify(I.edits.map(e => e.change)) !== '[{"add_labels":["bug"]},{"add_labels":["ui"]}]') throw new Error("not sent one name at a time, in order: " + JSON.stringify(I.edits));
      I.factEdit = "labels"; issuesSig = ""; drawIssues(); "ok"`,
    'denied': {
      run: `ghpRead(); window.__issues(Object.assign({act: "gh_project", ok: false, project: "shop", settings: true,
        error: T["err.ghproject.denied"]}, {seq: I.want.gh_project})); "ok"`,
      sizes: [['wide', 1280, 860]],
    },
  },
};
