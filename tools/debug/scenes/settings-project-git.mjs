/**
 * What git does in a project, on the project's own page, for
 * tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-project-git.mjs
 *
 * A desk with one project that guards two branches, and a folder of it. The
 * checkout is not on this machine, so the cards that read the repository
 * (git account, .gitignore, environment) say so or stay away; what is judged
 * here is the protected branches and the prompts, which need no disk.
 */

const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';
// The project's page, then one card of it brought to the top, the way a
// link from the board lands there
const projectPage = '(async () => { sel = {desk:0, proj:"p:shop", grp:null, tab:null, global:false}; render(); await ' + wait(500) + '; })()';
const cardOf = (id) => '(async () => { await ' + projectPage + ';'
  + ' const c = document.getElementById("' + id + '"); if (c) c.scrollIntoView({block:"start"});'
  + ' window.scrollBy(0, -80); })()';

export default {
  config: {
    desks: [{
      name: 'work', id: 'work',
      projects: [{ name: 'shop', at: 'D:/work/shop', git: { protect: ['main', 'release/*'] } }],
      folders: [{
        name: 'shop', cwd: 'D:/work/shop', project: 'shop',
        tabs: [{ name: 'claude', id: 'claude', command: 'claude' }],
      }],
    }],
  },
  scenes: {
    // The page from its top: name, checkout, folders, then git
    project: projectPage,
    // The branches a commit will not land on, as the project says them
    protect: cardOf('project-protect'),
    // What the AI is told when it writes the commit message
    prompts: cardOf('project-git'),
    // A folder of the project: its own answer is off, so the box shows the
    // project's names greyed out
    folder: '(async () => { sel = {desk:0, grp:0, tab:null, global:false}; render(); await ' + wait(500) + '; })()',
    // The way the board's ✨ arrives: the folder named in the address, and
    // the page lands on the project's commit-message box, marked
    landing: { query: 'section=project-git-message&folder=D:/work/shop&desk=0', run: wait(1200) },
  },
};
