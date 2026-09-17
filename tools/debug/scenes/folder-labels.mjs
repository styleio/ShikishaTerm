/**
 * Folder names and summaries written from what the AIs were asked, for
 * tools/debug/shoot.mjs. How a scene file is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/folder-labels.mjs
 *
 * One project with its checkout and three worktrees: one named by a person,
 * one whose name and summary were written for it (Auto, drawn a shade quieter),
 * and one just made under Auto, still called by its branch. What is judged is
 * whether an automatic name can be told from a chosen one without shouting,
 * whether a phone's card carries one line of the summary without crowding the
 * branch under it, and whether the menu a held press opens reads as the whole
 * summary over the choices. The state is written the way the app sends it
 * (uistate.rs).
 */

const site = 'D:/work/site/.git';
const tab = (index, name, group, extra) => Object.assign({
  index, name, id: name, state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 1, 3, 2, 0, 0, 1, 0, 0, 0], group, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);

const at = (leaf) => 'C:/Users/me/SHIKISHA-TERM/branches/site/' + leaf;
const fine = { health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } };

const groups = {
  en: [
    { name: 'site', folder: 'D:/work/site', color: '#4285f4', linked: false, family: site, branch: 'main', ...fine },
    { name: 'Pricing page redesign', folder: at('pricing'), color: '#4285f4', linked: true, family: site,
      branch: 'pricing', summary: 'Rebuild the pricing page around three plans, with a monthly and yearly switch and the comparison table moved under the cards.', ...fine },
    { name: 'Sign-in error messages', folder: at('mighty-gannet'), color: '#4285f4', linked: true, family: site,
      branch: 'mighty-gannet', auto: true,
      summary: 'Show why signing in failed — wrong password, locked account or expired link — instead of the blank form that comes back now.', ...fine },
    { name: 'quiet-otter', folder: at('quiet-otter'), color: '#4285f4', linked: true, family: site,
      branch: 'quiet-otter', auto: true, ...fine },
  ],
  ja: [
    { name: 'site', folder: 'D:/work/site', color: '#4285f4', linked: false, family: site, branch: 'main', ...fine },
    { name: '料金ページの作り直し', folder: at('pricing'), color: '#4285f4', linked: true, family: site,
      branch: 'pricing', summary: '料金ページを3つのプランを軸に作り直す。月額と年額の切り替えを付け、比較表はカードの下に移す。', ...fine },
    { name: 'ログインのエラー表示', folder: at('mighty-gannet'), color: '#4285f4', linked: true, family: site,
      branch: 'mighty-gannet', auto: true,
      summary: 'ログインに失敗したとき、空のフォームに戻るだけでなく、パスワード違い・ロック・期限切れのどれかを表示する。', ...fine },
    { name: 'quiet-otter', folder: at('quiet-otter'), color: '#4285f4', linked: true, family: site,
      branch: 'quiet-otter', auto: true, ...fine },
  ],
};

const state = (lang, active) => JSON.stringify({
  desk: 'work', desk_id: 'work', desks: ['work'], desk_index: 0, active,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: groups[lang],
  tabs: [
    tab(0, 'shell', 0),
    tab(1, 'claude', 1, { ai: 'claude', state: 'DONE', state_label: 'Done' }),
    tab(2, 'codex', 2, { ai: 'codex', state: 'BUSY', state_label: 'Working' }),
    tab(3, 'claude', 3, { ai: 'claude' }),
  ],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [{ key: 'claude', label: 'Claude Code' }], assistant: 'claude',
});

// The language the page was opened in decides which names it is handed
const put = (active) => `window.__state(document.documentElement.lang === "ja"`
  + ` ? ${JSON.stringify(state('ja', active))} : ${JSON.stringify(state('en', active))});`;

// The menu a held press (or a right-click) opens on the card written for
const menu = `const row = [...document.querySelectorAll(".tab.folder.wcard")][2];`
  + ` const r = row.getBoundingClientRect();`
  + ` folderMenu({currentTarget: row, clientX: r.left + 40, clientY: r.bottom}, S.groups[2]); "ok"`;

export default {
  settle: 1500,
  scenes: {
    // A person's name, a written one, and one still waiting to be written
    cards: put(2) + ' "ok"',
    // What resting the pointer says, written into the page for the photograph
    about: put(2) + ` document.title = folderAbout(S.groups[2]); "ok"`,
    // The phone's drawer: a line of the summary on each card that has one
    drawer: {
      run: put(2) + ` document.getElementById("app").classList.add("drawer"); "ok"`,
      sizes: [['phone', 390, 820]],
    },
    // The whole summary over what can be done to the folder
    menu: { run: put(2) + ' ' + menu, sizes: [['wide', 1280, 860]] },
    menuphone: {
      run: put(2) + ` document.getElementById("app").classList.add("drawer"); ` + menu,
      sizes: [['phone', 390, 820]],
    },
    // The worktree dialog, opened on Auto
    dialog: put(0) + ` openBranch(S.groups[0]); "ok"`,
  },
};
