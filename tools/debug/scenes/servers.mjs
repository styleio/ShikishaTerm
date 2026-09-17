/**
 * The name a person gives a server ("Production", "Staging"), wherever it is
 * worn, for tools/debug/shoot.mjs. How a scene file is written is at the top
 * of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/servers.mjs
 *
 * One desk: an AI, a shell on production reached through its bastion, a shell
 * on staging, and a file panel on production -- the two servers named, the
 * production one asking for its name to be typed before anything that cannot
 * be undone. The state is written the way the app sends it (uistate.rs).
 */

const prod = { name: 'Production', color: '#e5644d', careful: true,
  machine: 'gw-prod.example.com:22>10.0.0.5:22' };
const staging = { name: 'Staging', color: '#12b3a8', machine: 'staging.example.com:22' };

const tab = (index, name, extra) => Object.assign({
  index, name, id: name, state: 'WAIT', state_label: 'Waiting', profile: 'GENERIC',
  locked: false, depth: 0, activity: [0, 1, 3, 2, 0, 0, 1, 0, 0, 0], group: 0, kind: 'pty',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: 'tab:' + index,
}, extra);

const state = (active, more) => JSON.stringify(Object.assign({
  desk: 'site', desk_id: 'site', desks: ['site'], desk_index: 0, active,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [{ name: 'site', folder: 'D:/work/site', color: '#4285f4', linked: false,
    family: 'D:/work/site/.git', branch: 'main',
    health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } }],
  tabs: [
    tab(1, 'claude', { ai: 'claude', state: 'BUSY', state_label: 'Working', auto: true }),
    tab(2, 'deploy', { mark: prod }),
    tab(3, 'deploy-stg', { mark: staging }),
    tab(4, 'files', { kind: 'sftp', state: 'SFTP', state_label: 'Files', restartable: false, mark: prod }),
  ],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, build: '', help_rows: [], ais: [],
}, more));

// Two panes, production beside staging: the moment a mark is for
const panes = JSON.stringify({
  single: false, focus: 1,
  panes: [
    { id: 1, x: 0, y: 0, w: 0.5, h: 1, surface: 2, focused: true },
    { id: 2, x: 0.5, y: 0, w: 0.5, h: 1, surface: 3, focused: false },
  ],
  dividers: [{ i: 0, x: 0.5, y: 0, w: 0, h: 1, ratio: 0.5, down: false }],
});

// The file panel's two sides, as files.mjs has them
const files = `
  F.server = "deploy@10.0.0.5:22";
  F.local.root = "D:/work/site/dist"; F.local.at = "D:/work/site/dist";
  F.remote.root = "/var/www/site"; F.remote.at = "/var/www/site";
  F.local.rows = [
    {name:"index.html", dir:false, size:24710, modified:1789000000},
    {name:"app.8f21c4.js", dir:false, size:1462300, modified:1789000000}];
  F.remote.rows = [
    {name:"index.html", dir:false, size:19004, modified:1787000000},
    {name:"app.1a0b7e.js", dir:false, size:1201900, modified:1787000000}];`;

const onPanel = 'window.__state(' + JSON.stringify(state(4)) + ');' + files + ' drawSftp();';

export default {
  // The page is a large one to parse, and a scene run against a page still
  // loading finds none of its functions
  settle: 1500,
  scenes: {
    // The sidebar, the tabs over the panes, and two captions side by side
    board: 'window.__state(' + JSON.stringify(state(2)) + ');'
      + ' putTabsAway("D:/work/site", false);'
      + ' window.__panes(' + JSON.stringify(panes) + '); "ok"',
    // A set of tabs put away still says which servers are in it
    folded: 'window.__state(' + JSON.stringify(state(1)) + ');'
      + ' putTabsAway("D:/work/site", true); "ok"',
    // The file panel: the connection's bar and the server's own column
    panel: onPanel + ' "ok"',
    // Deleting on a server that asks for its name: waiting for it...
    remove: onPanel + ' askQuestion({title:T["sftp.remove.title"], say:T["sftp.remove.say"],'
      + ' what: sftpWhere("remote", rjoin(F.remote.at, "index.html")), mark: sftpMark(), sure: true,'
      + ' label:T["sftp.remove"], danger:true, go(){}}); "ok"',
    // ...pressed before it was typed, which says why and points at the box...
    pressed: {
      run: onPanel + ' askQuestion({title:T["sftp.remove.title"], say:T["sftp.remove.say"],'
        + ' what: sftpWhere("remote", rjoin(F.remote.at, "index.html")), mark: sftpMark(), sure: true,'
        + ' label:T["sftp.remove"], danger:true, go(){}}); sAskGo(); "ok"',
      looks: ['dark'],
    },
    // ...and typed, which lets the button go
    typed: {
      run: onPanel + ' askQuestion({title:T["sftp.remove.title"], say:T["sftp.remove.say"],'
        + ' what: sftpWhere("remote", rjoin(F.remote.at, "index.html")), mark: sftpMark(), sure: true,'
        + ' label:T["sftp.remove"], danger:true, go(){}});'
        + ' const i = document.getElementById("ssq"); i.value = "production";'
        + ' i.dispatchEvent(new Event("input")); "ok"',
      looks: ['light'],
      sizes: [['wide', 1280, 860]],
    },
    // A send that replaces files there, with its list
    send: onPanel + ' F.local.sel = new Set(["index.html", "app.8f21c4.js"]); sftpSend("local"); "ok"',
  },
};
