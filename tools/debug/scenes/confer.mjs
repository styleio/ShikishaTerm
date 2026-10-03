/**
 * AIConfer, the conversation panel's chat of AIs asking each other things,
 * for tools/debug/shoot.mjs.
 *
 * A desk of three AI tabs: the person names two of them, one asks the other
 * for a review twice, the answers come back as lines of their own, marks are
 * put on them, a commit and a pull request are shared as cards, a decision is
 * made, and one answer has no line of its own (its first sentence is shown).
 * Answered the way the app answers the panel, through `window.__convo`.
 */

const now = Date.UTC(2026, 9, 1, 5, 30);
const at = (min) => now - min * 60000;
const deskUid = 'b7e166dc-1190-4347-ad31-2cf0b9235a11';
const tabUid = name => ({otter:'3a3f5e2d-f411-47fa-8191-7b724d38c113',
  finch:'07b6cb08-2492-4515-b0c3-8e9d3a902934', heron:'b84e2c32-e0a1-41a4-b3d6-0668b65bf6ec'})[name];

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, uid: tabUid(name), state: 'DONE', state_label: 'Done', profile: 'Claude Code',
  locked: false, depth: 0, activity: [0, 1, 3, 2, 0, 0, 1, 0, 0, 0], group: 0, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: true, key: 'tab:' + index,
}, extra);

const state = JSON.stringify({
  desk: 'work', desk_id: 'work', desk_uid: deskUid, desks: ['work'], desk_index: 0, active: 1,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    { name: 'shop', folder: 'D:/work/shop', color: '#5b7cff', linked: false, family: 'D:/work/shop/.git', branch: 'fix/tax',
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
    { name: 'shop-staging', folder: 'D:/work/shop-staging', color: '#5b7cff', linked: false, family: 'D:/work/shop/.git', branch: 'staging',
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs: [
    tab(1, 'otter', { ai: 'claude', state: 'BUSY', state_label: 'Working' }),
    tab(2, 'finch', { ai: 'codex', profile: 'Codex CLI' }),
    tab(3, 'heron', { ai: 'claude', group: 1, state: 'QUESTION', state_label: 'Waiting for you' }),
  ],
  jobs: [],
  confer: { rev: 7, open: 0, open_desk: deskUid, auto_open: true, line_max: 80, max_rounds: 3 },
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
});

const setup = `
  window.__state(${JSON.stringify(state)});
  setSideWidth(400);
  sideReveal("convo");
  convoModeTo("confer");
  "ok"`;

const ask = (id, caller, target, text, reply, round) =>
  ({ id, caller, target, text, reply, state: reply ? 'DONE' : 'waiting', round });
const a1 = ask(1, 'otter', 'finch', 'Review the tax band cache in src/pricing.rs on branch fix/tax. Say what is missing.',
  '## Review\n\nTwo things.\n\n1. The cache never expires: a band changed in the admin page is not seen until a restart.\n2. There is no test for a discounted line.', 1);
const a2 = ask(2, 'otter', 'finch', 'I added a 10 minute expiry and the test. Look again at the same diff.', 'Looks good now: the expiry and the test are both there.', 2);
const a3 = ask(3, 'otter', 'heron', 'Deploy fix/tax to staging and time the checkout page.', 'The staging deploy finished and the checkout answers in 90 ms. The slowest query is now the cart itself.', 3);

const said = [
  { k: 'line', id: 1, tab: null, at: at(52), how: 'person', ask: null, marks: [],
    text: '<@otter> cache the tax bands, then have <@finch> review it until nothing is left' },
  { k: 'line', id: 2, tab: 'otter', at: at(31), how: 'ask', ask: a1, marks: [], text: 'Could you review the tax band cache?' },
  { k: 'line', id: 3, tab: 'finch', at: at(27), how: 'said', ask: a1, text: 'Two things: the cache never expires, and a test is missing.',
    marks: [{ by: 'otter', mark: '👀' }] },
  { k: 'line', id: 4, tab: 'otter', at: at(18), how: 'ask', ask: a2, marks: [], text: 'Fixed both. Mind a second look?' },
  { k: 'line', id: 5, tab: 'finch', at: at(16), how: 'said', ask: a2, text: 'Looks good now. Ship it.',
    marks: [{ by: 'otter', mark: '👍' }, { by: 'person', mark: '🎉' }, { by: 'heron', mark: '👍' }] },
  { k: 'share', id: 1, tab: 'otter', at: at(15), kind: 'commit', target: 'a1b2c3d4e5f6', title: 'Cache the tax bands for ten minutes',
    detail: { short: 'a1b2c3d', branch: 'fix/tax' } },
  { k: 'line', id: 6, tab: 'otter', at: at(14), how: 'agreed', ask: null, marks: [], text: 'Expire the cache after 10 minutes? → yes' },
  { k: 'line', id: 7, tab: 'otter', at: at(6), how: 'ask', ask: a3, marks: [], text: 'Can you deploy it to staging and time checkout?' },
  { k: 'line', id: 8, tab: 'heron', at: at(2), how: 'auto', ask: a3, marks: [],
    text: 'The staging deploy finished and the checkout answers in 90 ms.' },
  { k: 'share', id: 2, tab: 'otter', at: at(1), kind: 'pr', target: 'https://github.com/acme/shop/pull/42', title: 'Cache the tax bands',
    detail: { host: 'github.com', number: 42 } },
];

// The conversations otter (the tab in front) took part in: this one, and an
// earlier one about something else
const threads = [
  { id: 1, last_at: at(1), tabs: ['otter', 'finch', 'heron'], first: said[0].text },
  { id: 2, last_at: at(240), tabs: ['otter', 'heron'], first: 'Could you check why the staging build is slow?' },
];
const answer = (msg) => `window.__convo(Object.assign({panel: "confer", ok: true, desk: "${deskUid}", req: CF.seq["${msg.act}"]}, ${JSON.stringify(msg)})); "ok"`;
const listed = (list) => answer({ act: 'confer_threads', tab: tabUid('otter'), threads: list });
const page = (rows) => answer({ act: 'confer', thread: 1, said: rows, more: false }) + `;
  if (CF.said.length !== ${rows.length} || !cvUi.list.textContent.includes("Ship it."))
    throw new Error("The saved conference was not drawn");
  const requestsBefore = convoSerial;
  drawConvo();
  if (requestsBefore !== convoSerial) throw new Error("Drawing a loaded conference requested it again");
  "ok"`;

export default {
  setup,
  scenes: {
    chat: listed(threads) + ';' + page(said),
    remote: {served: 'remote', run: listed(threads) + ';' + page(said)},
    // The whole answer opened under a line
    full: listed(threads) + ';' + page(said)
      + `; document.querySelectorAll('#convopanel .cffold .vmore')[1].click(); "ok"`,
    // The other conversations this tab took part in, offered
    threads: listed(threads) + ';' + page(said) + `; document.querySelector('#convopanel .cfthpick').click(); "ok"`,
    // Nothing yet: what the panel says instead
    empty: listed([]),
  },
};
