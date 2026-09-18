/**
 * The list a tab that came up on a conversation of nobody's offers, for
 * tools/debug/shoot.mjs.
 *
 * Drawn from a state written here, so nothing has to be running and no records
 * have to exist. Where the offer itself lives -- a mark in the pane's caption
 * and one on the tab's own row -- is photographed against a running copy
 * instead (`tools/debug/past-back.win.mjs`): both are drawn as a tab starts,
 * and what is behind them is a terminal.
 */

const setup = `
  S = {
    tabs: [{index:1, kind:"pty", id:"claude", name:"claude", state:"WAIT",
            state_label:"WAIT", profile:"Claude Code", ai:"claude",
            restartable:true, past:true, activity:[]}],
    active: 1, desk: "Work", desks: ["Work"], desk_index: 0, groups: [],
  };
  "ok"`;

// What the folder's records hold, as the app hands them over: what was asked
// first in each conversation, and when it was last written
const hits = (now) => [
  { program: 'claude', id: '2222', title: 'shop', when: now - 15 * 60,
    snippet: 'say which of the two was wrong when a sign-in fails' },
  { program: 'claude', id: '1111', title: 'shop', when: now - 3 * 3600,
    snippet: 'take an email address on the login page instead of a user name' },
];

export default {
  setup,
  scenes: {
    list: `
      S.past = {tab:1, name:"claude", hits: ${JSON.stringify(hits(Math.floor(Date.now() / 1000)))}};
      window.__openPast(1, "claude");
      renderPast();
      "ok"`,
  },
};
