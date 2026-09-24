/**
 * The quick actions screen, for tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-actions.mjs
 *
 * A list read down in the bar's order, and a dialog to make or change one,
 * as a desk's secrets are. What is judged: whether each row says what the
 * button is called, whether it types or runs, and what it sends; whether a
 * Lua that does not parse is said on its row; whether the dialog holds its
 * save and says why; and whether a row carried by its grip lands where it
 * was let go. The carrying scene drives the page's own pointer handlers, so
 * the picture after it is the order really written.
 */

const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';
const open = 'sel = {desk:0, tab:null, global:true, section:"actions"}; render(); await ' + wait(700) + ';';

// A press on a row's grip, carried to the middle of another row, and let go
// there unless `hold` says to stop on the way
const carry = (from, to, hold) => '(async () => {' + open
  + 'const grip = document.querySelector(\'.arow[data-at="' + from + '"] .agrip\');'
  + 'const g = grip.getBoundingClientRect();'
  + 'const at = (x, y) => ({pointerId: 1, pointerType: "mouse", button: 0, clientX: x, clientY: y, bubbles: true});'
  + 'grip.dispatchEvent(new PointerEvent("pointerdown", at(g.left + 8, g.top + 8)));'
  + 'const t = document.querySelector(\'.arow[data-at="' + to + '"]\').getBoundingClientRect();'
  + 'const y = t.top + t.height * 0.5, x = t.left + Math.min(40, t.width / 2);'
  + 'for (let k = 1; k <= 8; k++) { dispatchEvent(new PointerEvent("pointermove", at(g.left + 8 + (x - g.left - 8) * k / 8, g.top + 8 + (y - g.top - 8) * k / 8))); await ' + wait(30) + '; }'
  + (hold ? '' : 'dispatchEvent(new PointerEvent("pointerup", at(x, y))); await ' + wait(600) + ';')
  + '})()';

// The dialog of the row at `at` (or a new one), with the fields filled as
// `fill` says, and its save pressed when `press` says so
const dialog = (at, fill, press) => '(async () => {' + open
  + (at === null ? 'document.querySelector("#actionslist").parentElement.querySelector(".row button").click();'
                 : 'document.querySelector(\'.arow[data-at="' + at + '"]\').click();')
  + 'await ' + wait(300) + ';'
  + (fill || '')
  + (press ? 'document.querySelector(".modal .mfoot .primary").click(); await ' + wait(600) + ';' : '')
  + '})()';

export default {
  config: {
    desks: [{
      name: 'site', id: 'site',
      folders: [{ name: 'site', cwd: 'D:/work/site', tabs: [{ name: 'shell', command: 'cmd' }] }],
    }],
    actions: [
      { label: 'Continue', body: 'Please continue.' },
      { label: 'Explain', body: 'Explain what you just did, briefly.' },
      { label: 'Review', body: 'Review this and point out any problems.' },
      { label: 'To reviewer', body: 'shikisha.send_to_tab("reviewer", tab.output)', lua: true },
      { label: 'Broken', body: 'shikisha.send_to_tab("reviewer", tab.output', lua: true },
      { label: 'Fix', body: 'Fix that and show me the change.' },
    ],
  },
  scenes: {
    // Every action, read down in the bar's order; the Lua that does not
    // parse says so on its row
    list: '(async () => {' + open + '})()',
    // One of them opened: a text action
    dialog: dialog(1),
    // A Lua action opened: the box is monospace and named for what it runs
    'dialog-lua': dialog(3),
    // A new one, saved with nothing in it: held, and the reason on the foot
    held: dialog(null, '', true),
    // The Lua that does not parse, its save pressed: held on the Lua's own
    // verdict, said under the box
    'held-lua': dialog(4, '', true),
    // A row lifted by its grip, on its way past the others
    carrying: carry(5, 1, true),
    // The same row put down where it was carried: the order really written
    carried: carry(5, 1, false),
  },
};
