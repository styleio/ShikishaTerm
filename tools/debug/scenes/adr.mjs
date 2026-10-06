/**
 * The decision-record (ADR) panel and its form, for tools/debug/shoot.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/adr.mjs
 *
 * An AI tab in a folder of a project that keeps records, the column open on
 * the ADR panel, and the app's answers handed to the page the way the app
 * hands them (window.__adr), so no folder and no AI are needed. Records are
 * written in Japanese as well as English: a title in any script is a title.
 */

const records = [
  { file: '0001-record-architecture-decisions.md', number: 1, title: 'Record architecture decisions',
    status: 'accepted', date: '2026-01-05', makers: 'Aiko', fits: false, text: 'We will use ADRs.' },
  { file: '0002-use-postgresql.md', number: 2, title: 'Use PostgreSQL for orders', status: 'superseded',
    by: '0004-注文の保存に-sqlite-を使う.md', date: '2026-02-11', makers: 'Aiko, Ben', fits: true, text: 'PostgreSQL' },
  { file: '0003-キャッシュに-redis-を使う.md', number: 3, title: 'キャッシュに Redis を使う', status: 'accepted',
    date: '2026-03-02', makers: '佐藤, 鈴木', fits: true, text: 'Redis Memcached' },
  { file: '0004-注文の保存に-sqlite-を使う.md', number: 4, title: '注文の保存に SQLite を使う', status: 'proposed',
    date: '2026-10-06', makers: '佐藤', fits: true, text: 'ＳＱＬｉｔｅ' },
  { file: '0005-drop-the-xml-export.md', number: 5, title: 'Drop the XML export', status: 'rejected',
    date: '2026-10-01', makers: 'Ben', fits: true, text: 'XML' },
];
const fields = [
  ['Context and Problem Statement', 2, false], ['Decision Drivers', 2, true], ['Considered Options', 2, false],
  ['Decision Outcome', 2, false], ['Consequences', 3, true], ['Confirmation', 3, true],
  ['Pros and Cons of the Options', 2, true], ['More Information', 2, true],
].map(([heading, level, optional]) => ({ heading, level, optional, hint: '' }));

const text = [
  '---', 'status: proposed', 'date: 2026-10-06', 'decision-makers: 佐藤', 'consulted: Claude Code (AI)', '---', '',
  '# 注文の保存に SQLite を使う', '',
  '## Context and Problem Statement', '', '注文は1日数百件で、PostgreSQL のサーバーを運用し続ける費用が見合わない。', '',
  '## Considered Options', '', '* SQLite', '* PostgreSQL のまま', '',
  '## Decision Outcome', '', 'SQLite を選ぶ。運用するサーバーが要らないため。', '',
  '### Consequences', '', '* 良い点: サーバーが要らない', '* 悪い点: 書き込みが1か所に限られる', '',
  '## More Information', '', 'Supersedes [ADR-0002](0002-use-postgresql.md).', '',
].join('\n');

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, state: "DONE", state_label: "Done", profile: "Claude Code",
  locked: false, depth: 0, activity: [0, 1, 3, 2, 0, 0, 1, 0, 0, 0], group: 0, kind: "pty",
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: true, key: "tab:" + index,
}, extra);

const state = JSON.stringify({
  desk: "work", desk_id: "work", desks: ["work"], desk_index: 0, active: 1,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    { name: "shop", folder: "D:/work/shop", key: "D:/work/shop", color: "#5b7cff", linked: false, family: "D:/work/shop/.git",
      branch: "main", whole: "shop", adr: "docs/decisions", health: { as: "fine" }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs: [tab(1, "claude", { ai: "claude" })],
  jobs: [],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: "", progress: 0, awaiting_human: false },
  auto_enabled: true, build: "", help_rows: [], ais: [],
});

const setup = `
  window.__state(${JSON.stringify(state)});
  setSideWidth(380);
  sideChoose("adr");
  window.__adr({act:"list", panel:"claude", ok:true, dir:"docs/decisions", exists:true,
    records:${JSON.stringify(records)}, fields:${JSON.stringify(fields)}, template:null});
  "ok"`;

const read = (file, status) => `window.__adr({act:"read", panel:"claude", ok:true, dir:"docs/decisions",
  file:${JSON.stringify(file)}, mark:"m",
  text:${JSON.stringify(text)}.replace("status: proposed", "status: " + ${JSON.stringify(status)}),
  record:Object.assign({}, AD.records.find(r => r.file === ${JSON.stringify(file)}), {status:${JSON.stringify(status)}}),
  parts:{title:"注文の保存に SQLite を使う",
    front:[{key:"decision-makers", value:"佐藤"}, {key:"consulted", value:"Claude Code (AI)"}],
    sections:[{heading:"Context and Problem Statement", body:"注文は1日数百件で、PostgreSQL のサーバーを運用し続ける費用が見合わない。"},
      {heading:"Considered Options", body:"* SQLite\\n* PostgreSQL のまま"}]},
  fields:AD.fields}); "ok"`;

export default {
  setup,
  scenes: {
    // The records, newest first, and one narrowed by a full-width search
    list: '"ok"',
    search: 'AD.q = "sqlite"; AD.rev++; drawAdr(); "ok"',
    empty: 'window.__adr({act:"list", panel:"claude", ok:true, dir:"docs/decisions", exists:false, records:[], fields:AD.fields}); "ok"',
    // A proposal read, and a decision that stands
    proposed: read('0004-注文の保存に-sqlite-を使う.md', 'proposed'),
    accepted: read('0004-注文の保存に-sqlite-を使う.md', 'accepted'),
    // The grey Edit of a standing decision, answering with its two ways
    acceptededit: read('0004-注文の保存に-sqlite-を使う.md', 'accepted').replace('"ok"', '')
      + 'adrEditAccepted(AD.read); "ok"',
    // The AI's answer, over the list
    answer: 'AD.answer = {q:"なぜ PostgreSQL をやめたの？", text:"ADR-0004 で、注文の保存を SQLite に移すことを提案しています（まだ提案中です）。ADR-0002 の PostgreSQL は ADR-0004 に置き換えられました。"}; AD.rev++; drawAdr(); "ok"',
    // The form, new and empty; and filled in by a draft
    form: 'adrNew(null); "ok"',
    drafted: 'adrNew(null); AD.busy = "draft";'
      + ' window.__adr({act:"draft", panel:"claude", ok:true, consulted:"Claude Code (AI)", data:JSON.stringify({title:"注文の保存に SQLite を使う",'
      + ' sections:{"Context and Problem Statement":"注文は1日数百件。", "Considered Options":"* SQLite\\n* PostgreSQL", "Decision Outcome":"SQLite を選ぶ。"}})}); "ok"',
    // A save refused, the form kept
    refused: 'adrNew(null); document.querySelector("#sask .brow > .go").click(); "ok"',
    // What a proposal will run, asked before it runs
    propose: 'window.__adr({act:"plan", panel:"claude", ok:true, file:"0004-注文の保存に-sqlite-を使う.md", title:"注文の保存に SQLite を使う",'
      + ' path:"docs/decisions/0004-注文の保存に-sqlite-を使う.md", branch:"adr-0004", message:"ADR-0004 を提案: 注文の保存に SQLite を使う", pr:"ADR-0004: 注文の保存に SQLite を使う"}); "ok"',
  },
};
