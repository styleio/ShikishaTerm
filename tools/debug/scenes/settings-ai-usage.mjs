/**
 * The AI allowance card, for tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-ai-usage.mjs
 *
 * Claude's and Codex's allowance, each with its 5-hour and 7-day window: a
 * bar, how much is used and when it comes back, and for Codex how old the
 * reading is. The readings are this PC's own, so what is judged is whether
 * each AI says either its numbers or, when it has none, why and what to do.
 */

const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';

export default {
  config: {
    desks: [{
      name: 'site', id: 'site',
      folders: [{ name: 'site', cwd: 'D:/work/site', tabs: [{ name: 'shell', command: 'cmd' }] }],
    }],
  },
  scenes: {
    // Opened: both AIs read as they stand (Claude's is a request, so it waits)
    usage: '(async () => { sel = {desk:0, tab:null, global:true, section:"aiusage"}; render();'
      + ' await ' + wait(7000) + '; window.scrollTo(0, 0); })()',
  },
};
