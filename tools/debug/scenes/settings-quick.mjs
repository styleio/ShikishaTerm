/**
 * The quick commands screen, for tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-quick.mjs
 *
 * A list read down in the launcher's order, and a dialog to make or change
 * one, as a desk's secrets are. What is judged: whether each row says what the
 * button is and sends, whether the pages of the launcher show as boxes once
 * there is more than one, whether a folder is walked into and out of, whether
 * the dialog holds its save and says why, and whether a row carried by its
 * grip lands where it was let go -- among the rows, into a folder, and out of
 * one through the path above. The carrying scenes drive the page's own
 * pointer handlers, so the pictures after them are the order really written.
 */

const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';
const open = 'sel = {desk:0, tab:null, global:true, section:"quick"}; render(); await ' + wait(700) + ';';

// A press on a row's grip, carried to the middle of another row (or a name
// in the path), and let go there unless `hold` says to stop on the way
const carry = (fromId, toSel, where, hold) => '(async () => {' + open
  + 'const grip = document.querySelector(\'.qrow[data-id="' + fromId + '"] .qgrip\');'
  + 'const g = grip.getBoundingClientRect();'
  + 'const at = (x, y) => ({pointerId: 1, pointerType: "mouse", button: 0, clientX: x, clientY: y, bubbles: true});'
  + 'grip.dispatchEvent(new PointerEvent("pointerdown", at(g.left + 8, g.top + 8)));'
  + 'const t = document.querySelector(\'' + toSel + '\').getBoundingClientRect();'
  + 'const y = t.top + t.height * ' + where + ', x = t.left + Math.min(40, t.width / 2);'
  + 'for (let k = 1; k <= 8; k++) { dispatchEvent(new PointerEvent("pointermove", at(g.left + 8 + (x - g.left - 8) * k / 8, g.top + 8 + (y - g.top - 8) * k / 8))); await ' + wait(30) + '; }'
  + (hold ? '' : 'dispatchEvent(new PointerEvent("pointerup", at(x, y))); await ' + wait(600) + ';')
  + '})()';

const cmd = (id, page, row, col, label, body, extra) =>
  Object.assign({ id, page, row, col, label, body, icon: '' }, extra || {});

export default {
  config: {
    desks: [{
      name: 'site', id: 'site',
      folders: [{ name: 'site', cwd: 'D:/work/site', tabs: [{ name: 'shell', command: 'cmd' }] }],
    }],
    quick_commands: {
      cols: 4, rows: 3,
      items: [
        cmd('test', 0, 0, 0, 'Run tests', 'npm test', { icon: 'test-tube' }),
        cmd('build', 0, 0, 1, 'Build', 'npm run build', { icon: 'hammer' }),
        cmd('review', 0, 0, 2, 'Review', 'Review the changes since the last commit and list what to fix first.',
          { icon: 'sparkles', kind: 'ai', ai: 'claude' }),
        { id: 'deploy', page: 0, row: 0, col: 3, label: 'Deploy', icon: 'rocket', kind: 'folder', items: [
          cmd('stg', 0, 0, 1, 'Staging', 'npm run deploy -- --env staging', { icon: 'cloud-upload' }),
          cmd('prd', 0, 0, 2, 'Production', 'npm run deploy -- --env production', { icon: 'cloud' }),
        ] },
        cmd('lint', 0, 1, 0, 'Lint', 'npm run lint -- --fix', { icon: 'wand-sparkles' }),
        cmd('db', 0, 1, 1, 'Open the database', 'psql "postgres://app:{' + '{secrets.db_pass}' + '}@localhost/app"', { icon: 'database' }),
        cmd('log', 0, 1, 2, 'Git log', 'git log --oneline -20', { icon: 'git-commit-horizontal' }),
        cmd('pr', 0, 1, 3, 'Draft a PR', 'Write a pull request description for this branch.', { icon: 'git-pull-request', kind: 'ai' }),
        cmd('status', 0, 2, 0, 'Status', 'git status', { enter: false }),
        cmd('clean', 1, 0, 0, 'Clean', 'git clean -fdx', { icon: 'trash-2' }),
        cmd('docs', 1, 0, 1, 'Docs', 'Summarise what changed in docs/ this week.', { icon: 'file-text', kind: 'ai', ai: 'codex' }),
      ],
    },
  },
  scenes: {
    // The list, in the launcher's order, one box per page
    list: '(async () => {' + open + ' window.scrollTo(0, 0); })()',
    // Inside a folder: the path back up, and the folder's own dialog
    folder: '(async () => {' + open
      + 'document.querySelector(\'.qrow[data-id="deploy"]\').click(); await ' + wait(300) + '; window.scrollTo(0, 0); })()',
    // A button being changed; its body names a secret
    edit: '(async () => {' + open
      + 'document.querySelector(\'.qrow[data-id="db"]\').click(); await ' + wait(400) + '; })()',
    // A new one saved with nothing in it: held, and saying why
    held: '(async () => {' + open
      + '[...document.querySelectorAll(".qadd button")][0].click(); await ' + wait(300) + ';'
      + '[...document.querySelectorAll(".modal")].pop().querySelector(".primary").click(); await ' + wait(300) + '; })()',
    // A row on its way down the list
    carrying: carry('test', '.qrow[data-id="log"]', 0.8, true),
    // ...let go below "Git log": Run tests is fourth on page 1 after it
    dropped: carry('test', '.qrow[data-id="log"]', 0.8, false),
    // ...let go over the middle of the folder: it is inside, last
    into: carry('lint', '.qrow[data-id="deploy"]', 0.5, false)
      .replace(/\}\)\(\)$/, 'document.querySelector(\'.qrow[data-id="deploy"]\').click(); await ' + wait(300) + '; })()'),
    // ...and out again, over the name of the top in the path
    out: '(async () => {' + open
      + 'document.querySelector(\'.qrow[data-id="deploy"]\').click(); await ' + wait(300) + ';'
      + 'const grip = document.querySelector(\'.qrow[data-id="stg"] .qgrip\'); const g = grip.getBoundingClientRect();'
      + 'const at = (x, y) => ({pointerId: 1, pointerType: "mouse", button: 0, clientX: x, clientY: y, bubbles: true});'
      + 'grip.dispatchEvent(new PointerEvent("pointerdown", at(g.left + 8, g.top + 8)));'
      + 'const c = document.querySelector(".qhere .crumb").getBoundingClientRect();'
      + 'dispatchEvent(new PointerEvent("pointermove", at(c.left + 10, c.top + c.height / 2))); await ' + wait(50) + ';'
      + 'dispatchEvent(new PointerEvent("pointerup", at(c.left + 10, c.top + c.height / 2))); await ' + wait(600) + ';'
      + 'document.querySelector(".qhere .crumb").click(); await ' + wait(300) + '; window.scrollTo(0, 0); })()',
  },
};
