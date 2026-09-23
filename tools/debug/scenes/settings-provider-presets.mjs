/**
 * Adding a model connection from the list of known services, for
 * tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-provider-presets.mjs
 *
 * A desk that already has a connection called "jev", so the one picked from
 * the list has to take the next free name. The scenes: the dialog as it
 * opens, a decision model picked, a conversation model picked, a model on
 * this PC picked, and the decision model saved with its model name.
 */
const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';
const open = '(async () => { providerDialog(desks[0], null, () => {}); await ' + wait(300) + '; })()';
const pick = (label) => '(async () => { await ' + open + ';'
  + ' const s = document.querySelector(".modal:last-of-type .mbody select");'
  + ' const o = [...s.options].find(o => o.textContent.startsWith(' + JSON.stringify(label) + '));'
  + ' if (!o) throw new Error("no such service on the list: ' + label + '");'
  + ' s.value = o.value; s.dispatchEvent(new Event("change")); await ' + wait(200) + '; })()';

export default {
  config: {
    desks: [{
      name: 'Work', id: 'work',
      providers: { jev: { base_url: 'https://api.typesafe.ai/v1/systemone', speaks: 'choice' } },
      folders: [],
    }],
  },
  scenes: {
    empty: open,
    decision: pick('Jev'),
    conversation: pick('DeepSeek'),
    here: pick('Ollama'),
    // Saved: the decision model's name comes along, so the picker for the
    // model that chooses a page's next move can offer "jev-2/jev-latest"
    saved: '(async () => { await ' + pick('Jev') + ';'
      + ' document.querySelector(".modal:last-of-type .mfoot button.primary").click(); await ' + wait(300) + ';'
      + ' const got = JSON.stringify((desks[0].providers["jev-2"] || {}).models);'
      + ' if (got !== JSON.stringify(["jev-latest"])) throw new Error("models saved as " + got); })()',
  },
  langs: ['ja', 'en'],
  sizes: [['wide', 1280, 900], ['phone', 390, 820]],
};
