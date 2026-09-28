/**
 * A job one AI tab hands out to others, under the tab that leads it, for
 * tools/debug/shoot.mjs. How a scene file is written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/job.mjs
 *
 * The lead (claude) has had codex implement a fix and claude review it; the
 * review is under way, a second try at the fix is queued behind it, and a
 * decision -- merge to main? -- waits for the person. What is judged is
 * whether the card reads as belonging to the lead's row, whether each task's
 * state and the tab on it are read at a glance, and whether the decision
 * stands out as the one thing asking for the person. The state is written the
 * way the app sends it (uistate.rs, `jobs` from orch::Orchestra::board).
 */

const site = 'D:/work/site/.git';
const tab = (index, name, group, extra) => Object.assign({
  index, name, id: name, state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 1, 3, 2, 0, 0, 1, 0, 0, 0], group, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);

const job = (decide) => ({
  id: 1,
  objective: 'Fix the login redirect and get it reviewed',
  lead: 'lead',
  opened_at: 0,
  rounds: 3,
  working: true,
  workers: ['codex', 'reviewer'],
  tasks: [
    { id: 1, title: 'Fix the redirect after login in auth/session.rs', state: 'completed', tab: 'codex', tab_state: 'DONE', tries: 1 },
    { id: 2, title: 'Review the fix on branch fix-login', state: 'dispatched', tab: 'reviewer', tab_state: 'BUSY', tries: 1 },
    { id: 3, title: 'Fix what the review finds', state: 'pending', tab: null, tries: 0 },
    { id: 4, title: 'Merge fix-login into main', state: decide ? 'blocked' : 'ready', note: decide ? 'waiting for a decision (g1)' : null, tab: null, tries: 0 },
  ],
  decisions: decide ? [{ id: 1, task: 4, question: 'Merge fix-login into main now?', options: ['Merge', 'Not yet'], who: 'person' }] : [],
});

const state = (decide) => JSON.stringify({
  desk: 'work', desk_id: 'work', desks: ['work'], desk_index: 0, active: 1,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [
    { name: 'site', folder: 'D:/work/site', color: '#4285f4', linked: false, family: site, branch: 'main',
      health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } },
  ],
  tabs: [
    tab(1, 'lead', 0, { ai: 'claude', state: 'BUSY', state_label: 'Working' }),
    tab(2, 'codex', 0, { ai: 'codex', state: 'DONE', state_label: 'Done' }),
    tab(3, 'reviewer', 0, { ai: 'claude', state: 'BUSY', state_label: 'Working' }),
    tab(4, 'shell', 0),
  ],
  jobs: [job(decide)],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
});

export default {
  settle: 1500,
  scenes: {
    // Under way: the review is running, nothing asks for the person
    working: `window.__state(${JSON.stringify(state(false))}); "ok"`,
    // A decision waits for the person
    decide: `window.__state(${JSON.stringify(state(true))}); "ok"`,
    // The same on a phone, with the drawer the list lives in pulled out
    phone: {
      run: `window.__state(${JSON.stringify(state(true))}); document.getElementById("app").classList.add("drawer"); "ok"`,
      sizes: [['phone', 390, 820]],
    },
    // Stop, asked first
    stop: `window.__state(${JSON.stringify(state(false))});`
      + ` setTimeout(() => document.querySelector(".job .jhead button").click(), 300); "ok"`,
  },
};
