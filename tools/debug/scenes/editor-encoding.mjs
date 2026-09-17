/**
 * A file that is not UTF-8, open in the editor, for tools/debug/shoot.mjs. How
 * a scene file is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/editor-encoding.mjs
 *
 * The editor loads its library from vendor/ace beside the page, so copy it
 * there first: vendor/ace -> target/shots/vendor/ace.
 *
 * A spreadsheet's CSV saved as Shift_JIS. Opened, it reads as itself with its
 * encoding in the menu. A save with an emoji typed into it stops and names the
 * characters, with the two ways on. Read as UTF-8 instead, it says what was
 * lost, and a save stops to ask before writing the loss into the file. The
 * answers are written the way the app sends them (files_answer in runtime.rs).
 */

const family = 'D:/work/shop/.git';

const state = JSON.stringify({
  desk: 'shop', desk_id: 'shop', desks: ['shop'], desk_index: 0, active: 2,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    { name: 'shop', folder: 'D:/work/shop', color: '#4285f4', linked: false, family, branch: 'main',
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs: [
    { index: 2, name: 'editor', id: 'editor', state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
      locked: false, depth: 0, activity: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0], group: 0, kind: 'editor',
      model: false, busy: false, settings: false, auto: false, restartable: true, readable: false,
      key: 'tab:2', file: 'data/customers.csv' },
  ],
  making: [],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
});

const csv = [
  '顧客番号,氏名,住所,電話番号',
  '1001,山田太郎,東京都千代田区,03-1234-5678',
  '1002,佐藤花子,大阪府堺市,072-234-5678',
  '1003,鈴木一郎,愛知県名古屋市,052-345-6789',
].join('\r\n') + '\r\n';

const read = (text, encoding, exact) => ({
  act: 'read', ok: true, path: 'data/customers.csv', text, encoding, exact, mark: 'm', stamp: 's',
});
const open = (answer) => `window.__state(${JSON.stringify(state)});`
  + ` window.__files(${JSON.stringify(answer)});`;
// Waits for the editor library, then types and presses save
const later = (js) => ` (function go(n) { if (!edAce && n) return setTimeout(() => go(n - 1), 100); ${js} })(50); "ok"`;

export default {
  settle: 2500,
  scenes: {
    opened: open(read(csv, 'Shift_JIS', true)) + ' "ok"',
    unwritable: open(read(csv, 'Shift_JIS', true)) + later(
      'edAce.session.doc.setValue(' + JSON.stringify(csv + '1004,高橋美咲😀,福岡県福岡市,092-456-7890\r\n') + ');'
      + ' editSave();'
      + ' window.__files({act:"write", ok:false, why:"unwritable", encoding:"Shift_JIS", chars:["😀"], more:0, error:""});'),
    lossy: open(read(csv.replace(/[^\x00-\x7f]+/g, (m) => '\ufffd'.repeat(m.length * 2)), 'UTF-8', false)) + ' "ok"',
    lossysave: open(read(csv.replace(/[^\x00-\x7f]+/g, (m) => '\ufffd'.repeat(m.length * 2)), 'UTF-8', false))
      + later('editSave();'),
  },
};
