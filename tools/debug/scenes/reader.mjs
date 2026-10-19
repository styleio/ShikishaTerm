/**
 * The reader (📖), on the window and on a phone, for tools/debug/shoot.mjs.
 * How a scene file is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/reader.mjs
 *
 * One way in, the same on both: 📖 over the right edge of an AI tab's
 * terminal. On the window it stands alone (a wheel scrolls the terminal); on a
 * phone the pager's ▲▼ stand under it. A shell keeps no record to read, so it
 * has no 📖. Opened, the reader stands over the pane it reads on the window
 * and over the whole screen on a phone, and asks for the conversation down
 * the same line the column's conversation panel uses. Answered here the way
 * the app answers, through `window.__convo`, with what the app sends back:
 * the newest first, the work and the events between the things said -- which
 * the reader leaves out.
 *
 * Each scene measures what it is about and throws when it is not so, so a
 * picture of a broken screen is never taken for a good one.
 */

const now = Date.UTC(2026, 9, 19, 5, 30);
const at = (min) => now - min * 60000;
const REC = 'c0ffee00-2222-4222-8222-222222222222';

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, uid: 'uid-' + name, state: 'DONE', state_label: 'Done', profile: 'Claude Code',
  locked: false, depth: 0, activity: [0, 1, 3, 2, 0, 0, 1, 0, 0, 0], group: 0, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: true, key: 'tab:' + index,
}, extra);

const stateOn = (active) => JSON.stringify(JSON.stringify({
  desk: 'work', desk_id: 'work', desks: ['work'], desk_index: 0, active,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    { name: 'shop', folder: 'D:/work/shop', color: '#5b7cff', linked: false, family: 'D:/work/shop/.git', branch: 'fix/tax',
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs: [tab(1, 'coder', { ai: 'claude' }), tab(2, 'shell', { ai: null, readable: false, profile: 'GENERIC' })],
  jobs: [],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, remote_on: true, usage: null, build: '', help_rows: [], ais: [],
}));

// A long answer, of the kind whose head scrolls out of a full-screen terminal
const answer = [
  'The checkout page was slow because the cart total read every tax band from the database on each request.',
  '',
  '## What changed',
  '',
  '- The tax bands are read once and kept for five minutes',
  '- A discounted line is now tested',
  '',
  '```rust',
  'let bands = cache.bands_or(|| db.tax_bands())?;',
  'assert_eq!(total(&cart), 1080);',
  '```',
  '',
  '| Page | Before | After |',
  '|---|---|---|',
  '| Checkout | 640 ms | 90 ms |',
  '| Cart | 410 ms | 70 ms |',
].join('\n');

// Newest first, as `convo::read` sends them
const rows = [
  {k: 'say', who: 'ai', record: REC, at: 9000, when: at(1), text: answer},
  {k: 'work', record: REC, from: 6000, to: 8900, calls: 4},
  {k: 'say', who: 'you', record: REC, at: 5000, when: at(9), text: 'the checkout page is slow, find out why and fix it',
    from: {by: 'person', device: 'window', via: 'composer'}},
  {k: 'wait', when: at(12), ended: at(11), answered: at(11), by: 'person', device: 'window', via: 'keys'},
];

const box = (id) => `document.getElementById(${JSON.stringify(id)}).getBoundingClientRect()`;
const shown = (id) => `(getComputedStyle(document.getElementById(${JSON.stringify(id)})).display !== "none" && ${box(id)}.width > 0)`;
const must = (cond, why) => `if (!(${cond})) throw new Error(${JSON.stringify(why)});`;

const onTab = (active) => `window.__state(${stateOn(active)});
  window.__screen('<div class="r">\\u276f the checkout page is slow</div><div class="r">  \\u23bf  Read 3 files</div>');
  new Promise(ok => setTimeout(ok, 200))`;

// Every question the reader has out, answered with `reply`
const reply = (reply) => `for (const req of [...rdAsked.keys()]) window.__convo(Object.assign({panel: rdKey, act: "page", req}, ${JSON.stringify(reply)}));`;

const opened = (served) => `${onTab(1)}.then(() => {
  document.getElementById("readOpen").click();
  ${must('rdAsked.size === 1', 'opening the reader asked nothing')}
  ${reply({ok: true, rows, older: {record: REC, before: 900, until: at(12)}})}
  return new Promise(ok => setTimeout(ok, 150));
}).then(() => {
  ${must(shown('reader'), 'the reader did not open')}
  ${must('document.querySelectorAll("#rbody .rturn").length === 2', 'the reader did not keep only what was said')}
  ${must('document.querySelector("#rbody .rturn").classList.contains("you")', 'what was said is not oldest first')}
  ${must(shown('rearlier'), 'there is more to read, and no way back to it')}
  ${must('!' + shown('pageui'), 'the buttons stand over the reader')}
  ${served === 'window'
    ? must(`(() => { const r = ${box('reader')}, s = ${box('screen')};
        return Math.abs(r.left - s.left) < 1 && Math.abs(r.right - s.right) < 1 && Math.abs(r.top - s.top) < 1; })()`,
      'the reader does not stand over the pane it reads')
    : must(`(() => { const r = ${box('reader')}; return r.left === 0 && r.top === 0 && r.right === innerWidth; })()`,
      'the reader does not take the whole phone screen')}
  return "ok";
})`;

export default {
  scenes: {
    // 📖 alone on the window's AI tab
    'button-window': {
      served: 'window', sizes: [['wide', 1280, 860]],
      run: `${onTab(1)}.then(() => {
        ${must(shown('readOpen'), 'the window has no 📖 on an AI tab')}
        ${must('!' + shown('pageUp'), 'the window shows the phone pager')}
        return "ok"; })`,
    },
    // 📖 on the phone's AI tab. The pager under it is not judged here: this
    // page is given a stand-in for the window's bridge, which makes every page
    // the window's to it. reader.win.mjs looks at a real phone's
    'button-phone': {
      served: 'remote', sizes: [['phone', 390, 820]],
      run: `${onTab(1)}.then(() => {
        ${must(shown('readOpen'), 'the phone has no 📖 on an AI tab')}
        return "ok"; })`,
    },
    // A shell keeps no record: no 📖 on either
    'shell-window': {
      served: 'window', sizes: [['wide', 1280, 860]],
      run: `${onTab(2)}.then(() => {
        ${must('!' + shown('readOpen'), 'a shell offers a reader with nothing to read')}
        return "ok"; })`,
    },
    'open-window': { served: 'window', sizes: [['wide', 1280, 860]], run: opened('window') },
    'open-phone': { served: 'remote', sizes: [['phone', 390, 820]], run: opened('remote') },
    // Reading failed: the reason, and the way to read again
    'failed-window': {
      served: 'window', sizes: [['wide', 1280, 860]],
      run: `${onTab(1)}.then(() => {
        document.getElementById("readOpen").click();
        ${reply({ok: false, error: 'This tab is no longer open.'})}
        return new Promise(ok => setTimeout(ok, 150));
      }).then(() => {
        ${must('document.querySelector("#rbody .ragain")', 'a failed reading offers no way to read again')}
        return "ok"; })`,
    },
    // Esc puts it away, and the buttons come back
    'closed-window': {
      served: 'window', sizes: [['wide', 1280, 860]],
      run: `${opened('window')}.then(() => {
        document.dispatchEvent(new KeyboardEvent("keydown", {key: "Escape", bubbles: true}));
        ${must('!rdIsOpen()', 'Esc did not put the reader away')}
        window.__state(${stateOn(1)});
        ${must(shown('readOpen'), 'the buttons did not come back')}
        return "ok"; })`,
    },
  },
  settle: 1200,
};
