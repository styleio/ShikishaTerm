/**
 * Where a server is given its name, for tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-servers.mjs
 *
 * A desk with a file panel on production (behind its bastion), a shell on
 * staging, and a shell on a server nobody has named yet.
 */

const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';
// The tab's own page, scrolled to where the name is asked for. The name is
// read back from the app, so the drawing waits for that answer
const tabPage = (i) => '(async () => { sel = {desk:0, tab:' + i + ', global:false}; render();'
  + ' await ' + wait(700) + ';'
  + ' const m = document.querySelector(".markpart"); if (m) m.scrollIntoView({block:"start"});'
  + ' window.scrollBy(0, -80); })()';

export default {
  config: {
    server_marks: {
      'gw-prod.example.com:22>10.0.0.5:22': { name: 'Production', color: '#e5644d', careful: true },
      'staging.example.com:22': { name: 'Staging', color: '#12b3a8' },
    },
    desks: [{
      name: 'site', id: 'site',
      folders: [{
        name: 'site', cwd: 'D:/work/site',
        tabs: [
          { name: 'files', id: 'files', command: 'sftp://deploy@10.0.0.5:22',
            server: { jump: { host: 'gw-prod.example.com', user: 'me' }, remote_dir: '/var/www/site' } },
          { name: 'deploy-stg', id: 'deploy-stg', command: 'ssh://deploy@staging.example.com:22' },
          { name: 'db', id: 'db', command: 'ssh://deploy@db.example.com:22' },
        ],
      }],
    }],
  },
  scenes: {
    // A server named already: its name, colour and care, read from the settings
    named: tabPage(0),
    // A server being named: the colour and the care appear with the name
    naming: '(async () => { await ' + tabPage(2) + ';'
      + ' const i = document.querySelector(".markpart input[type=text]");'
      + ' i.value = "Database"; i.dispatchEvent(new Event("input")); })()',
    // Every named server, to find one again
    list: '(async () => { sel = {desk:0, tab:null, global:true, section:"servers"}; render(); })()',
    // One of them opened
    dialog: '(async () => { sel = {desk:0, tab:null, global:true, section:"servers"}; render();'
      + ' await ' + wait(300) + '; document.querySelector(".markname").closest(".listrow").click(); })()',
  },
};
