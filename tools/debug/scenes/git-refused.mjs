/**
 * A fetch the server refused as the account, for tools/debug/shoot.mjs. How a
 * scene file is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/git-refused.mjs
 *
 * The git column beside a folder whose project has chosen no account, so git
 * on this PC signed in as it does -- and GitHub would not let that account
 * into the repository. The column says which account was refused and what
 * the server said, with the way to the project's git account card under it.
 * A failure of any other kind is said as it came, with no button: the third
 * scene is the same fetch with the network down.
 */

const family = 'D:/work/shop/.git';

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0], group: 0, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);

const state = JSON.stringify({
  desk: 'shop', desk_id: 'shop', desks: ['shop'], desk_index: 0, active: 1,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    { name: 'shop', folder: 'D:/work/shop', color: '#4285f4', linked: false, family, branch: 'checkout-total',
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs: [tab(1, 'claude', { ai: 'claude' })],
  making: [],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
});

const branch = {
  name: 'checkout-total', upstream: 'origin/checkout-total', ahead: 1, behind: 0,
  protected: false, detached: false,
};

// The column open on the changes, with nothing to commit
const setup = `
  window.__state(${JSON.stringify(state)});
  sidePanel = "git";
  setSideWidth(420);
  window.__git(${JSON.stringify({ act: 'branch', ok: true, data: branch })});
  window.__git(${JSON.stringify({ act: 'branches', ok: true, data: [{ name: 'checkout-total', current: true }, { name: 'main', current: false }] })});
  window.__git(${JSON.stringify({ act: 'status', ok: true, data: [] })});
  "ok"`;

// What the app answers a fetch with, in the words it would use: the wording
// comes from the page's own dictionary, so the picture is in every language
const refused = (key, fill) => `(() => {
  let said = T[${JSON.stringify(key)}] || ${JSON.stringify(key)};
  for (const [k, v] of Object.entries(${JSON.stringify(fill)})) said = said.split("{" + k + "}").join(v);
  window.__git({act: "fetch", ok: false, why: "account", folder: "D:/work/shop", error: said});
  return "ok";
})()`;

export default {
  setup,
  settle: 1500,
  scenes: {
    // This PC's git, refused by the server: the button under the words
    pc: refused('err.git.pc_refused', { said: 'Write access to repository not granted' }),
    // This PC holds two GitHub accounts, and git could not tell which to use
    many: refused('err.github.pc_many', { names: 'octo-cat, octo-dog' }),
    // An account from the settings, refused under its own name
    account: refused('err.git.account.refused', { name: 'work', said: 'Repository not found' }),
    // The network, not the account: the same words git said, and no button
    down: `(() => {
      window.__git({act: "fetch", ok: false,
        error: "git fetch --prune failed: fatal: unable to access 'https://github.com/shop/shop.git/': Could not resolve host: github.com"});
      return "ok";
    })()`,
  },
};
