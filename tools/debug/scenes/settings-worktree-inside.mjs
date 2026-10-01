/**
 * A project's rules for places inside the folders a worktree inherits, for
 * tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-worktree-inside.mjs
 *
 * The project is this checkout itself, so the lines above the rules are the
 * real ones. One rule leaves the other agents' worktrees out of `.claude`;
 * the other is written the way a rule here cannot be, so the page says why.
 */
import path from 'node:path';

const ROOT = path.resolve(import.meta.dirname, '..', '..', '..');
const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';

export default {
  config: {
    desks: [{
      name: 'Check', id: 'check',
      folders: [{ name: 'ShikishaTerm', cwd: ROOT, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }],
      projects: [{ name: 'ShikishaTerm', at: ROOT, bring: [
        { path: '.claude/worktrees/', how: 'skip' },
        { path: '!target/keep/', how: 'copy' },
      ] }],
    }],
  },
  scenes: {
    // Opened at its own address, once the sizes are in (they draw the page
    // again when they arrive, which would move it)
    inside: {
      query: 'desk=0&section=project-inside&folder=' + encodeURIComponent(ROOT),
      run: '(async () => { for (let i = 0; i < 300; i++) {'
        + ' if (document.getElementById("project-inside") && document.querySelector("#project-bring .igsize")) break;'
        + ' await ' + wait(200) + '; }'
        + ' await ' + wait(1500) + ';'
        + ' document.getElementById("project-inside").scrollIntoView({block:"start"}); window.scrollBy(0, -16); })()',
    },
  },
};
