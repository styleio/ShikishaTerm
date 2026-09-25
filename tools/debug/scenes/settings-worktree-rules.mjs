/**
 * A project's worktree rules, for tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-worktree-rules.mjs
 *
 * The project is this checkout itself: a repository that is really there, so
 * the app can answer where a worktree would go and how large each ignored
 * thing is (its build folder is large, which is what the page has to say).
 * The walk through a new project to its first worktree is checked against
 * the running app by tools/debug/worktree-rules.win.mjs; these are pictures.
 */
import path from 'node:path';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';
const onRules = 'desk=0&section=project-rules&folder=' + encodeURIComponent(ROOT);
// Until the app has said where a worktree goes and how large things are
const settled = '(async () => { for (let i = 0; i < 100; i++) {'
  + ' if (document.querySelector("#placesaid code") && document.querySelector("#project-bring .igsize")) break;'
  + ' await ' + wait(200) + '; } })()';

export default {
  config: {
    desks: [{
      name: 'Check', id: 'check',
      folders: [{ name: 'ShikishaTerm', cwd: ROOT, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }],
    }],
  },
  scenes: {
    // A project just added: its rules, with the way on above them
    first: { query: 'desk=0&section=project-first&folder=' + encodeURIComponent(ROOT), run: settled },
    // The files it inherits, what each holds, and a link offered for what is large
    inherit: { query: onRules, run: '(async () => { await ' + settled + ';'
      + ' document.getElementById("project-bring").scrollIntoView({block:"start"}); window.scrollBy(0, -16); })()' },
    // What to tell the AI
    ai: { query: onRules, run: '(async () => { await ' + settled + ';'
      + ' Array.from(document.querySelectorAll("#project-bring button")).find(b => b.textContent === T["settings.inherit.ai.button"]).click(); })()' },
    // "Is this right?", with a line of a file before and after
    confirm: { query: onRules, run: '(async () => { await ' + settled + ';'
      + ' const desk = desks[sel.desk]; const p = deskProjects(desk).projects.find(x => x.key === sel.proj);'
      + ' inheritConfirm(desk, p, {ok: true, name: "feature-x", folder: document.querySelector("#placesaid code").textContent, lines: ['
      + '  {source: ".gitignore", pattern: "/target", how: "link", replace: [], reason: "Build output, rebuilt rather than edited."},'
      + '  {source: ".gitignore", pattern: "/.private/", how: "copy", replace: [], reason: "Local notes the project reads."}]});'
      + ' await ' + wait(600) + '; })()' },
    // The projects listed from the banner (the one floating list, floatMenu)
    pickers: { query: onRules, run: '(async () => { await ' + settled + ';'
      + ' document.querySelector(".projbanner").click(); })()' },
    // The program-wide part: where worktrees go, and what tells a project served where it stands
    global: '(async () => { sel = {desk:0, tab:null, global:true, section:"worktrees"}; render(); })()',
  },
};
