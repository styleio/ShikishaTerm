/**
 * A machine's entry with the section on what its AIs do while the app is
 * away, for tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-away.mjs
 *
 * A server with the bridge, set to keep its AIs for 8 hours; a MicroVM with
 * the bridge and nothing chosen yet; a server without the bridge, where the
 * choice cannot be made. What is judged is whether the section says, before
 * the choice, what keeping them means, what it costs, what happens to their
 * calls, and that tabs on one machine are not kept apart -- and whether the
 * choice is shown as it would be written.
 */

const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';
const open = (i) => '(async () => { sel = {desk:0, tab:null, global:true, section:"hosts"}; render(); await ' + wait(300) + ';'
  + ' hostDialog(' + i + ', () => {}); await ' + wait(600) + ';'
  + ' const b = [...document.querySelectorAll(".bridgecard")][1]; if (b) b.scrollIntoView({block:"start"}); })()';

export default {
  config: {
    hosts: [
      { name: 'build-box', at: 'ssh://me@build.example.com:22', keepalive: 30, away: { minutes: 480 } },
      { name: 'cloud', kind: 'e2b', template: 'base', minutes: 30 },
      { name: 'old-box', at: 'ssh://me@old.example.com:22', keepalive: 30 },
    ],
    bridges: ['build-box', 'cloud'],
    desks: [{ name: 'site', id: 'site', folders: [{ name: 'site', cwd: 'D:/work/site', tabs: [] }] }],
  },
  scenes: {
    server: open(0),
    microvm: open(1),
    nobridge: open(2),
  },
};
