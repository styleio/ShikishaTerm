/**
 * A new issue that follows the repository's issue template, for
 * tools/debug/shoot.mjs. How a scene file is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/issue-template.mjs
 *
 * The repository has two forms. First the form before one is chosen; then the
 * bug form chosen, its title start and label already on; then the AI's
 * answer read in, one required field it could not fill from the notes said
 * and pointed at; then a Markdown template's description.
 */

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, state: "WAIT", state_label: "Waiting", profile: "GENERIC",
  locked: false, depth: 0, activity: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0], group: 0, kind: "pty",
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: "tab:" + index,
}, extra);

const state = JSON.stringify({
  desk: "shop", desk_id: "shop", desks: ["shop"], desk_index: 0, active: 2,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    { name: "shop", folder: "D:/work/shop", color: "#4285f4", linked: false, family: "D:/work/shop/.git",
      branch: "main", health: { as: "fine" }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs: [tab(1, "claude", { ai: "claude" }), tab(2, "Issues", { kind: "issues" })],
  making: [],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: "", progress: 0, awaiting_human: false },
  auto_enabled: true, build: "", help_rows: [], ais: [],
});

// Read from two forms by issueform.rs, as the app hands them over
const templates = {"blank":true,"items":[{"file":"bug.yml","name":"不具合の報告","about":"動かない・おかしい動きを知らせる","title":"[不具合] ","labels":["bug"],"assignees":[],"fields":[{"kind":"markdown","key":"field_1","label":"","description":"","placeholder":"","value":"報告ありがとうございます。**分かる範囲で**書いてください。","render":"","multiple":false,"options":[],"ticks_needed":[],"default":null,"required":false,"min_length":null},{"kind":"dropdown","key":"area","label":"どの画面で起きましたか","description":"","placeholder":"","value":"","render":"","multiple":false,"options":["ログイン","注文一覧","注文の詳細","請求書","設定","管理画面","その他"],"ticks_needed":[],"default":null,"required":true,"min_length":null},{"kind":"input","key":"url","label":"画面のアドレス","description":"ブラウザの上に出ているアドレスです。","placeholder":"https://example.com/orders/123","value":"","render":"","multiple":false,"options":[],"ticks_needed":[],"default":null,"required":true,"min_length":null},{"kind":"textarea","key":"what","label":"起きたこと","description":"","placeholder":"何をしたら\n何が起きたか\n","value":"","render":"","multiple":false,"options":[],"ticks_needed":[],"default":null,"required":true,"min_length":null},{"kind":"textarea","key":"expect","label":"期待した動き","description":"","placeholder":"","value":"","render":"","multiple":false,"options":[],"ticks_needed":[],"default":null,"required":false,"min_length":null},{"kind":"input","key":"browser","label":"ブラウザ","description":"","placeholder":"Chrome 131","value":"","render":"","multiple":false,"options":[],"ticks_needed":[],"default":null,"required":false,"min_length":null}],"body":"","start":[null,[],"","","",""]},{"file":"request.yml","name":"改善・要望","about":"こうなってほしい、を知らせる","title":"[要望] ","labels":["enhancement"],"assignees":[],"fields":[{"kind":"textarea","key":"want","label":"ほしいこと","description":"","placeholder":"","value":"","render":"","multiple":false,"options":[],"ticks_needed":[],"default":null,"required":true,"min_length":null}],"body":"","start":[""]}],"problems":[]};
templates.items.push({ file: "idea.md", name: "Idea", about: "A rough idea", title: "[Idea] ", labels: ["enhancement"],
  assignees: [], fields: null, body: "## Problem\n\n<!-- What is hard today? -->\n\n## Proposal\n", start: [] });

const setup = `
  window.__state(${JSON.stringify(state)});
  window.__issues(${JSON.stringify({
    act: "projects", ok: true,
    projects: [{ name: "shop", dir: "D:/work/shop", at: "D:/work/shop", repo: "acme/shop", account: "" }],
    accounts: [], pc: [], gh: [], labels: {}, waits: { create: 50, create_pr: 50 },
  })});
  window.__issues(${JSON.stringify({ act: "list", ok: true, seq: null, kind: "issue", items: [], total: 0 })});
  window.__issues(${JSON.stringify({ act: "options", ok: true, project: "shop",
    data: { labels: ["bug", "enhancement", "ui"], assignees: ["mika"], templates } })});
  I.view = "create";
  Object.assign(I.create, issueBlank({project: "shop"}));
  issuesSig = ""; drawIssues(); "ok"`;

const chosen = `issueTemplateUse(I.create, "bug.yml");
  I.create.body = "注文一覧で絞り込みを変えると、合計金額が前の絞り込みのまま。Chrome。";
  issuesSig = ""; drawIssues(); "ok"`;

export default {
  setup,
  scenes: {
    "not-chosen": `"ok"`,
    "chosen": chosen,
    "drafted": chosen + `;
      I.busy = "draft";
      window.__issues(${JSON.stringify({ act: "draft", ok: true, data: JSON.stringify({
        file: "bug.yml", title: "[不具合] 絞り込みを変えても合計金額が変わらない", labels: ["bug"], assignee: null,
        values: [null, [1], "", "注文一覧で絞り込みを変えると、合計金額が前の絞り込みのままになる。", "", "Chrome"],
        missing: [{ at: 2, label: "画面のアドレス" }],
      }) })});
      "ok"`,
    // The field the notes did not fill, scrolled to: its warning box under it
    "drafted-field": chosen + `;
      I.busy = "draft";
      window.__issues(${JSON.stringify({ act: "draft", ok: true, data: JSON.stringify({
        file: "bug.yml", title: "[不具合] 絞り込みを変えても合計金額が変わらない", labels: ["bug"], assignee: null,
        values: [null, [1], "", "注文一覧で絞り込みを変えると、合計金額が前の絞り込みのままになる。", "", "Chrome"],
        missing: [{ at: 2, label: "画面のアドレス" }],
      }) })});
      document.querySelector("#issuespanel .field.bad").scrollIntoView({block: "center"});
      "ok"`,
    "markdown":`issueTemplateUse(I.create, "idea.md"); issuesSig = ""; drawIssues(); "ok"`,
  },
};
