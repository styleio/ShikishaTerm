/**
 * A machine's entry with the bridge section, for tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-bridge.mjs
 *
 * One server the person agreed to put the bridge on, and one MicroVM entry that
 * has not been asked. What is judged is whether the section says, before its
 * box, what the bridge does, where it goes, how big it is and how it comes off.
 */

const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';
const open = (i) => '(async () => { sel = {desk:0, tab:null, global:true, section:"hosts"}; render(); await ' + wait(300) + ';'
  + ' hostDialog(' + i + ', () => {}); await ' + wait(300) + ';'
  + ' const b = document.querySelector(".bridgecard"); if (b) b.scrollIntoView({block:"center"}); })()';

export default {
  config: {
    hosts: [
      { name: 'build-box', at: 'ssh://me@build.example.com:22', keepalive: 30 },
      { name: 'cloud', kind: 'e2b', template: 'base', minutes: 30 },
    ],
    bridges: ['build-box'],
    desks: [{ name: 'site', id: 'site', folders: [{ name: 'site', cwd: 'D:/work/site', tabs: [] }] }],
  },
  scenes: {
    server: open(0),
    microvm: open(1),
  },
};
