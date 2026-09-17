/**
 * A worktree deleted from its right-click, for tools/debug/shoot.mjs. How a
 * scene file is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/worktree-removal.mjs
 *
 * One project with its own checkout and one worktree left. A second worktree
 * is on its way out: first while it is being deleted, then after its folder
 * would not go, with the question that is asked then. The state is written the
 * way the app sends it (uistate.rs).
 */

const family = 'D:/work/site/.git';
const leaving = {
  id: 7, family, name: 'issue-3-tell-production-from-staging',
  folder: 'C:/Users/me/SHIKISHA-TERM/branches/site/issue-3-tell-production-from-staging',
};
const why = 'target\\shots\\chrome-profile\\Default\\Cache\\data_1 を削除できません: '
  + 'プロセスはファイルにアクセスできません。別のプロセスが使用中です。 (os error 32)';

const tab = (index, name, group, extra) => Object.assign({
  index, name, id: name, state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 1, 3, 2, 0, 0, 1, 0, 0, 0], group, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);

const state = (making) => JSON.stringify({
  desk: 'site', desk_id: 'site', desks: ['site'], desk_index: 0, active: 1,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    { name: 'site', folder: 'D:/work/site', color: '#4285f4', linked: false, family, branch: 'main',
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
    { name: 'login', folder: 'C:/Users/me/SHIKISHA-TERM/branches/site/login', color: '#4285f4', linked: true,
      family, branch: 'login', health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs: [
    tab(1, 'claude', 0, { ai: 'claude', state: 'BUSY', state_label: 'Working', auto: true }),
    tab(2, 'codex', 1, { ai: 'codex' }),
  ],
  making,
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
});

const lang = (ja, en) => `(document.documentElement.lang === "ja" ? ${JSON.stringify(ja)} : ${JSON.stringify(en)})`;
const withWhy = (m) => `Object.assign(${JSON.stringify(m)}, {error: ${lang(why, why
  .replace(' を削除できません: ', ' could not be deleted: ')
  .replace('プロセスはファイルにアクセスできません。別のプロセスが使用中です。',
    'The process cannot access the file because it is being used by another process.'))}})`;

export default {
  settle: 1500,
  scenes: {
    // Its row says it is going, and offers nothing that cannot be done
    removing: `window.__state(${JSON.stringify(state([{ ...leaving, stage: 'removing' }]))}); "ok"`,
    // The folder would not go: the row keeps its reason and three answers,
    // and the question is put once
    unremoved: `const s = JSON.parse(${JSON.stringify(state([]))}); s.making = [Object.assign(${withWhy({ ...leaving, stage: 'unremoved' })})];`
      + ' window.__state(JSON.stringify(s)); "ok"',
    // Cancel: the question goes, the row stays to be answered later
    cancelled: `const s = JSON.parse(${JSON.stringify(state([]))}); s.making = [Object.assign(${withWhy({ ...leaving, stage: 'unremoved' })})];`
      + ' window.__state(JSON.stringify(s)); closeAsk(); window.__state(JSON.stringify(s)); "ok"',
  },
};
