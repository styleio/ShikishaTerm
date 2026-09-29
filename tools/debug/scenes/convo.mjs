/**
 * The column's conversation panel, for tools/debug/shoot.mjs.
 *
 * One AI tab's conversation, the newest at the top: what the person sent from
 * this PC and from afar, what another tab and a job sent, the AI's replies,
 * a stretch of tool runs opened, a wait for an answer and who gave it, a stop,
 * and where the conversation began. Then a search whose matches are partly in
 * kinds put away, and a tab nothing has been said in yet. Answered the way the
 * app answers them, through `window.__convo`, rather than by reading records
 * that are not here.
 */

const now = Date.UTC(2026, 8, 29, 5, 30);
const at = (min) => now - min * 60000;
const REC = 'c0ffee00-1111-4111-8111-111111111111';

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, state: 'DONE', state_label: 'Done', profile: 'Claude Code',
  locked: false, depth: 0, activity: [0, 1, 3, 2, 0, 0, 1, 0, 0, 0], group: 0, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: true, key: 'tab:' + index,
}, extra);

const state = JSON.stringify({
  desk: 'work', desk_id: 'work', desks: ['work'], desk_index: 0, active: 1,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    { name: 'shop', folder: 'D:/work/shop', color: '#5b7cff', linked: false, family: 'D:/work/shop/.git', branch: 'fix/tax',
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs: [tab(1, 'coder', { ai: 'claude' }), tab(2, 'lead', { ai: 'codex', state: 'BUSY', state_label: 'Working' })],
  jobs: [],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
});

const setup = `
  window.__state(${JSON.stringify(state)});
  sidePanel = "convo";
  CV.show = {you: true, aiin: true, ai: true, work: true, events: true};
  setSideWidth(380);
  "ok"`;

const rows = [
  {k: 'say', who: 'ai', record: REC, at: 9000, when: at(1),
    text: 'Pushed to `fix/tax`. The checkout page answers in about 90 ms now.'},
  {k: 'stop', when: at(3), by: 'person', device: 'phone', how: 'esc'},
  {k: 'say', who: 'you', record: REC, at: 8000, when: at(4), text: 'push it', pin: true,
    note: 'The push that closed the tax ticket',
    from: {by: 'person', device: 'phone', via: 'composer'}},
  {k: 'wait', when: at(9), ended: at(6), answered: at(6), by: 'person', device: 'window', via: 'keys'},
  {k: 'say', who: 'ai', record: REC, at: 7000, when: at(10),
    text: 'The review found one missing test for a discounted line; I added it.\n\n```rust\nassert_eq!(total(&cart), 1080);\n```'},
  {k: 'work', record: REC, from: 6000, to: 6900, calls: 3},
  {k: 'say', who: 'you', record: REC, at: 5000, when: at(14), text: 'Review the change in src/pricing.rs and say what is missing.',
    from: {by: 'tab', via: 'ask', sender: 'lead'}},
  {k: 'say', who: 'you', record: REC, at: 4000, when: at(20), text: 'You are working on task t2 of job 3: cache the tax bands.',
    from: {by: 'job', via: 'brief', sender: 'lead', job: 3}},
  {k: 'say', who: 'ai', record: REC, at: 3000, when: at(26), text: 'The cart total reads every tax band from the database on each request.'},
  {k: 'say', who: 'you', record: REC, at: 1000, when: at(30), text: 'the checkout page is slow',
    from: {by: 'person', device: 'window', via: 'composer'}},
  {k: 'begin', record: REC, when: at(30), yolo: false},
];

const work = {pieces: [
  {kind: 'call', name: 'Bash', text: 'cargo test pricing'},
  {kind: 'out', text: 'running 12 tests\ntest total_with_discount ... FAILED', after: 4210},
  {kind: 'call', name: 'Edit', text: 'src/pricing.rs'},
]};

const answer = (msg) => `window.__convo(Object.assign({panel: "coder", ok: true}, ${JSON.stringify(msg)})); "ok"`;

export default {
  setup,
  scenes: {
    chat: answer({act: 'page', req: 'page#1', rows, older: {record: REC, before: 900, until: at(30)}})
      + `; document.querySelector('#convopanel .vwork .vmore').click();`
      + answer({act: 'work', req: 'work:w' + REC + '@6000#1', record: REC, from: 6000, work, q: ''}),
    // Words typed: two things said hold them, and a tool run does too -- with
    // tool runs put away, the line at the foot says so and offers to show them
    search: `CV.show.work = false;`
      + answer({act: 'page', req: 'page#1', rows, older: null})
      + `; const q = document.querySelector('#convopanel .fsearch input'); q.value = 'tax'; q.dispatchEvent(new Event('input'));`
      // The search the page asks a moment after the typing stops, asked now
      + `; clearTimeout(cvTimer); convoAsk("find", {q: "tax", pins: false}); ${answer({act: 'find', req: 'find#1', q: 'tax', pins: false, capped: false,
        rows: [rows[2], {...rows[5], hit: true}, rows[7], rows[8]]})}`,
    empty: answer({act: 'page', req: 'page#1', rows: [], older: null}),
  },
  settle: 1200,
};
