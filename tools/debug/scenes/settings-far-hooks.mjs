/**
 * A machine's entry with the AI status reports (hooks) section, for
 * tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-far-hooks.mjs
 *
 * One server where one AI was allowed and one was refused, and one MicroVM
 * entry nobody has been asked about yet. What is judged is whether the
 * section says what the tick does, how it comes off, and -- for a machine not
 * asked about -- when it will be asked.
 */

const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';
const open = (i, card) => '(async () => { sel = {desk:0, tab:null, global:true, section:"hosts"}; render(); await ' + wait(300) + ';'
  + ' hostDialog(' + i + ', () => {}); await ' + wait(300) + ';'
  + ' const b = [...document.querySelectorAll(".bridgecard")][' + card + ']; if (b) b.scrollIntoView({block:"center"}); })()';

export default {
  config: {
    hosts: [
      { name: 'build-box', at: 'ssh://me@build.example.com:22', keepalive: 30 },
      { name: 'cloud', kind: 'e2b', template: 'base', minutes: 30 },
    ],
    far_hooks: { 'build-box': { 'Claude Code': 'on', 'Codex CLI': 'off' } },
    desks: [{ name: 'site', id: 'site', folders: [{ name: 'site', cwd: 'D:/work/site', tabs: [] }] }],
  },
  scenes: {
    server: open(0, 1),
    microvm: open(1, 1),
    // Removing the server that still carries an entry: taken out first, or left
    drop: '(async () => { await ' + open(0, 1) + '; await ' + wait(200) + ';'
      + ' const d = [...document.querySelectorAll(".mfoot button.danger")].pop(); if (d) d.click(); })()',
  },
};
