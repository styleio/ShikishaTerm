/**
 * The models a page is driven with in plain words, for
 * tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-words.mjs
 *
 * A desk with a decision model and a conversation model registered, whose
 * settings file still has the models written the old way (app-wide), and a
 * browser tab of its own. The scenes: the desk's Browser page (the old
 * models moved onto it), a browser tab following the desk, the tab with a
 * decision model put where the writer goes (warned, and the save refused),
 * a model added from the picker's last line (it lands selected, on the
 * desk too when the desk had none), and an AI installed on this PC chosen
 * from the top of the list (marked as a subscription, with its own models
 * offered and blank meaning its own).
 */
const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';
const deskPage = '(async () => { sel = {desk:0, grp:null, tab:null, global:false}; goDeskSection("browser", "start"); await ' + wait(300) + '; })()';
const tabPage = '(async () => { const i = desks[0].tabs.findIndex(t => t.id === "shop");'
  + ' sel = {desk:0, grp:0, tab:i, global:false}; render(); await ' + wait(300) + ';'
  + ' document.getElementById("tab-words").scrollIntoView({block:"start"}); })()';
const pickers = 'document.querySelectorAll("#tab-words select")';

export default {
  config: {
    operate: { choose_model: 'jev/jev-latest', words_model: 'deepseek/deepseek-flash' },
    desks: [{
      name: 'Work', id: 'work',
      providers: {
        jev: { base_url: 'https://api.typesafe.ai/v1/systemone', speaks: 'choice', models: ['jev-latest'] },
        deepseek: { base_url: 'https://api.deepseek.com', models: ['deepseek-flash'] },
      },
      folders: [{ tabs: [{ id: 'shop', name: 'Shop', command: 'browser https://example.com' }] }],
    }],
  },
  scenes: {
    // What the board opens when 🗣 is used on a page with no models: that
    // page's settings, at its models
    landed: { query: 'desk=0&tabkey=shop&section=words',
      run: '(async () => { await ' + wait(300) + ';'
        + ' const t = desks[0].tabs[sel.tab];'
        + ' if (!t || t.id !== "shop") throw new Error("landed on " + JSON.stringify(sel));'
        + ' const w = document.getElementById("tab-words").getBoundingClientRect();'
        + ' if (w.top < 0 || w.top > innerHeight) throw new Error("the models are not in view");'
        + ' if (!document.querySelector("#tab-words .warn")) throw new Error("the sheet does not say why it opened"); })()' },
    // The old app-wide models, moved onto the desk that had none
    desk: '(async () => { await ' + deskPage + ';'
      + ' const b = desks[0].browser;'
      + ' if (b.choose_model !== "jev/jev-latest" || b.words_model !== "deepseek/deepseek-flash")'
      + '   throw new Error("the old models did not move to the desk: " + JSON.stringify(b));'
      + ' if ((current.operate || {}).choose_model) throw new Error("the old models stayed app-wide"); })()',
    // A browser tab that says nothing of its own follows the desk
    tab: tabPage,
    // A decision model where the writer goes: warned where it is, and refused
    wrong: '(async () => { await ' + tabPage + ';'
      + ' const s = ' + pickers + '[1];'
      + ' if ([...s.options].some(o => o.value === "jev")) throw new Error("a decision model is offered as the writer");'
      + ' const t = desks[0].tabs.find(t => t.id === "shop"); t.words_model = "jev/jev-latest"; render(); await ' + wait(200) + ';'
      + ' await save(); await ' + wait(300) + ';'
      + ' if (!document.querySelector("#tab-words .site-warn")) throw new Error("no warning under the writer");'
      + ' document.getElementById("tab-words").scrollIntoView({block:"start"}); })()',
    // The last line of the picker: a connection added there lands selected
    added: '(async () => { await ' + tabPage + ';'
      + ' desks[0].browser = {};'
      + ' render(); await ' + wait(200) + '; document.getElementById("tab-words").scrollIntoView({block:"start"});'
      + ' const s = ' + pickers + '[1];'
      + ' s.value = "+add"; s.dispatchEvent(new Event("change")); await ' + wait(300) + ';'
      + ' const box = document.querySelector(".modal:last-of-type");'
      + ' const pre = box.querySelector(".mbody select");'
      + ' pre.value = [...pre.options].find(o => o.textContent.startsWith("OpenAI")).value; pre.dispatchEvent(new Event("change"));'
      + ' const model = [...box.querySelectorAll("input.mono")].find(i => i.style.flex); model.value = "gpt-6-sol";'
      + ' box.querySelector(".mfoot button.primary").click(); await ' + wait(400) + ';'
      + ' const t = desks[0].tabs.find(t => t.id === "shop");'
      + ' if (t.words_model !== "openai/gpt-6-sol") throw new Error("not selected: " + t.words_model);'
      + ' if (desks[0].browser.words_model !== "openai/gpt-6-sol") throw new Error("the desk did not take it");'
      + ' document.getElementById("tab-words").scrollIntoView({block:"start"}); })()',
    // The AIs installed here head the list, marked, and each is chosen with
    // no model (its own) or with one of the names every account has
    subscription: '(async () => { await ' + deskPage + ';'
      + ' const s = document.querySelector("#desk-words select");'
      + ' const first = s.querySelector("optgroup");'
      + ' if (!first || !first.querySelector("option[value=\\"@claude\\"]")) throw new Error("the installed AIs are not first: " + (first && first.label));'
      + ' s.value = "@claude"; s.dispatchEvent(new Event("change")); await ' + wait(200) + ';'
      + ' if (desks[0].browser.choose_model !== "@claude") throw new Error("written as " + desks[0].browser.choose_model);'
      + ' const chip = [...document.querySelectorAll("#desk-words button")].find(b => b.textContent === "haiku");'
      + ' chip.click(); await ' + wait(200) + ';'
      + ' if (desks[0].browser.choose_model !== "@claude/haiku") throw new Error("written as " + desks[0].browser.choose_model);'
      + ' document.getElementById("desk-words").scrollIntoView({block:"start"}); })()',
    // A desk that chose nothing drives its pages with the assistant AI, and
    // says so; without a decision model it is [Slow], and pressing that says
    // why and lights the field that fixes it
    slow: '(async () => { await ' + deskPage + ';'
      + ' desks[0].browser = {}; render(); await ' + wait(200) + ';'
      + ' const s = document.querySelector("#desk-words select");'
      + ' if (!s.options[0].textContent.includes("Claude Code")) throw new Error("the unset choice reads " + s.options[0].textContent);'
      + ' const tag = document.querySelector("#desk-words button.speedchip");'
      + ' if (!tag) throw new Error("no [Slow] to press");'
      + ' tag.click(); await ' + wait(150) + ';'
      + ' if (document.querySelector("#desk-words .site-warn").hidden) throw new Error("pressing [Slow] said nothing");'
      + ' if (!s.classList.contains("lookhere")) throw new Error("the field that fixes it did not light up");'
      + ' document.getElementById("desk-words").scrollIntoView({block:"start"}); })()',
    // A decision model deciding is [Fast], and there is nothing to press
    fast: '(async () => { await ' + deskPage + ';'
      + ' const s = document.querySelector("#desk-words select");'
      + ' s.value = "jev"; s.dispatchEvent(new Event("change")); await ' + wait(200) + ';'
      + ' if (document.querySelector("#desk-words button.speedchip")) throw new Error("still slow with Jev deciding");'
      + ' if (!document.querySelector("#desk-words span.speedchip")) throw new Error("no [Fast]");'
      + ' document.getElementById("desk-words").scrollIntoView({block:"start"}); })()',
  },
  langs: ['ja', 'en'],
  sizes: [['wide', 1280, 900], ['phone', 390, 820]],
};
