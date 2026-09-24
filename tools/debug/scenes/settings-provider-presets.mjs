/**
 * Adding a model connection from the list of known services, for
 * tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-provider-presets.mjs
 *
 * A desk that already has a connection called "jev", so the one picked from
 * the list has to take the next free name. The scenes: the dialog as it
 * opens, a decision model picked, a conversation model picked, a model on
 * this PC picked, the decision model saved with its model name, and the
 * limit on a decision model's choices (saved, then refused at 1).
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
    // A decision model has a limit on its choices, blank for the usual one;
    // a conversation model has no such field. Written only when given
    choices: '(async () => { await ' + pick('DeepSeek') + ';'
      + ' const box = () => [...document.querySelectorAll(".modal:last-of-type input[type=number]")].find(i => i.placeholder === "255");'
      + ' if (!box() || !box().closest(".field").hidden) throw new Error("a conversation model offers a limit on choices");'
      + ' await ' + pick('Jev') + ';'
      + ' const c = box(); if (c.closest(".field").hidden) throw new Error("no limit on choices for a decision model");'
      + ' c.value = "1000"; c.dispatchEvent(new Event("input"));'
      + ' c.closest(".field").scrollIntoView({block:"center"});'
      + ' document.querySelector(".modal:last-of-type .mfoot button.primary").click(); await ' + wait(300) + ';'
      + ' if ((desks[0].providers["jev-2"] || {}).max_choices !== 1000) throw new Error("saved as " + JSON.stringify(desks[0].providers["jev-2"]));'
      + ' await ' + pick('Jev') + ';'
      + ' const d = box(); d.value = "1"; d.dispatchEvent(new Event("input")); await ' + wait(100) + ';'
      + ' d.closest(".field").scrollIntoView({block:"center"}); })()',
  },
  langs: ['ja', 'en'],
  sizes: [['wide', 1280, 900], ['phone', 390, 820]],
};
