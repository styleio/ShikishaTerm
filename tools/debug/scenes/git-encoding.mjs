/**
 * A change to a file that is not UTF-8, read in an editor tab, for
 * tools/debug/shoot.mjs. How a scene file is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/git-encoding.mjs
 *
 * A spreadsheet's CSV saved as Shift_JIS, changed in two places. First as the
 * app reads it on its own (the encoding worked out, and said on "Auto"), then
 * as it reads when somebody has chosen UTF-8 for it: the characters come out as
 * replacement marks, which is said above the pieces, and the buttons that would
 * write them into the file are left out. The pieces are written the way the app
 * sends them (git_hunks in hooks.rs).
 */

const family = 'D:/work/shop/.git';

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
    { name: 'shop', folder: 'D:/work/shop', color: '#4285f4', linked: false, family, branch: 'main',
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs: [
    tab(1, 'claude', { ai: 'claude' }),
    tab(2, 'editor', { kind: 'editor', file: 'data/customers.csv', file_diff: 'work' }),
  ],
  making: [],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
});

const head = 'diff --git a/data/customers.csv b/data/customers.csv\n'
  + 'index 3f1a2b4..9c0d7e1 100644\n--- a/data/customers.csv\n+++ b/data/customers.csv\n';
const hunk = (header, start, end, lines) => ({
  file: 'data/customers.csv', header, start, end, patch: head + header + '\n' + lines.join('\n') + '\n',
});

// As the app reads it: Shift_JIS, exactly
const read = [
  hunk('@@ -1,5 +1,5 @@', 1, 5, [
    ' 顧客番号,氏名,住所,電話番号',
    ' 1001,山田太郎,東京都千代田区,03-1234-5678',
    '-1002,佐藤花子,大阪府大阪市,06-2345-6789',
    '+1002,佐藤花子,大阪府堺市,072-234-5678',
    ' 1003,鈴木一郎,愛知県名古屋市,052-345-6789',
    ' 1004,高橋美咲,福岡県福岡市,092-456-7890',
  ]),
  hunk('@@ -40,3 +40,4 @@', 40, 43, [
    ' 1039,伊藤健太,北海道札幌市,011-567-8901',
    ' 1040,渡辺真理,宮城県仙台市,022-678-9012',
    '+1041,中村陽子,広島県広島市,082-789-0123',
  ]),
].map((h) => ({ ...h, encoding: 'Shift_JIS', exact: true }));

// ...and as it reads with UTF-8 chosen: what is not ASCII is lost
const lost = read.map((h) => ({
  ...h,
  patch: h.patch.replace(/[^\x00-\x7f]+/g, (m) => '\ufffd'.repeat(Math.ceil(m.length * 1.5))),
  encoding: 'UTF-8',
  exact: false,
}));

const show = (hunks, chosen) => `window.__state(${JSON.stringify(state)});`
  + (chosen ? ` gitEncs[gitEncKey()] = ${JSON.stringify(chosen)};` : '')
  + ` window.__git(${JSON.stringify({ act: 'hunks', ok: true, data: hunks })});`
  + ` window.__git(${JSON.stringify({ act: 'diff', ok: true, data: 'changed' })}); "ok"`;

export default {
  settle: 1500,
  scenes: {
    auto: show(read, ''),
    lossy: show(lost, 'UTF-8'),
  },
};
