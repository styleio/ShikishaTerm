/**
 * The sign-in step of an AI on a MicroVM checkout, in each state it passes
 * through, for tools/debug/shoot.mjs. How a scene file is written is at the
 * top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/microvm-login.mjs
 *
 * What is judged: the title is the only line above the terminal; under the
 * keys is one guide that says the one thing to do now -- the terminal's
 * questions, then the address and the code field only while an address is
 * up, then what is left after the sign-in, then done; and "Next" is grey
 * until the sign-in is done, and blue only then.
 */

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0], group: 0, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);
const key = '\u0001microvm(E2b)\u0001/home/user/site';
const screens = {
  choose: '<div>Select login method:</div><div>❯ 1. Claude account with subscription</div><div>  2. Anthropic Console account</div>',
  url: '<div>Browser didn\'t open? Use the url below to sign in:</div><div>https://claude.ai/oauth/authorize?code=true&amp;client_id=abc</div><div>Paste code here if prompted &gt;</div>',
  notes: '<div>Security notes:</div><div>Press Enter to continue…</div>',
};
const state = (st, screen, url) => JSON.stringify({
  desk: 'ops', desk_id: 'ops', desks: ['ops'], desk_index: 0, active: 1,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  hosts: [{ name: 'microvm(E2b)', at: '', kind: 'microvm' }],
  groups: [{ name: 'site', key, folder: '/home/user/site', host: 'microvm(E2b)', color: '#4285f4', linked: false,
    family: 'microvm(E2b):/home/user/site/.git', branch: 'main', health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } }],
  tabs: [tab(1, 'claude', { ai: 'claude' })],
  making: [],
  login_step: { seq: 1, folder: '/home/user/site', key, host: 'microvm(E2b)', ai: 'claude', name: 'Claude Code',
    state: st, screen, url: url || '' },
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
});
const at = (st, screen, url) => `window.__state(${JSON.stringify(state(st, screen, url))}); "ok"`;

export default {
  settle: 1200,
  scenes: {
    choose: at('no', screens.choose),
    url: at('no', screens.url, 'https://claude.ai/oauth/authorize?code=true&client_id=abc'),
    finishing: at('finishing', screens.notes),
    done: at('yes', ''),
  },
};
