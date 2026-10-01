/**
 * The server versions this PC is paired with, for tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-boards.mjs
 *
 * One server added (its key is not on this PC in the shot, so its row says it
 * cannot be asked), and the form to add another. What is judged is whether the
 * card says what a server version is, how to add one (the command on the
 * server, the address, the code) and that the key is kept in the secrets.
 */

const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';

export default {
  config: {
    boards: [{ name: 'VPS1', url: 'https://vps1.example.ts.net:8787' }],
    desks: [{ name: 'site', id: 'site', folders: [{ name: 'site', cwd: 'D:/work/site', tabs: [] }] }],
  },
  scenes: {
    boards: '(async () => { sel = {desk:0, tab:null, global:true, section:"boards"}; render(); await ' + wait(1500) + '; })()',
  },
};
