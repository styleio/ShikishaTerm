/**
 * The models a page is driven with in plain words, for
 * tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-words.mjs
 *
 * Settings with a decision model and a conversation model registered, a
 * deciding AI chosen for the app, and a desk with a browser tab. The scenes:
 * the AI agents page (the assistant AI, the deciding AI, and the providers
 * with the installed AIs ahead of them), a browser tab following the app, the
 * tab with a decision model put where the writer goes (warned, and the save
 * refused), a deciding model added from the picker's last line (it lands
 * selected, on the app too when the app had none), an AI installed on this PC
 * chosen from the top of the list (marked as a subscription, with its own
 * models offered and blank meaning its own), and a connection's row opened,
 * where what it may be sent is agreed to.
 */
const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';
const aiPage = '(async () => { sel = {desk:0, tab:null, global:true, section:"ai"}; render(); await ' + wait(300) + '; })()';
const tabPage = '(async () => { const i = desks[0].tabs.findIndex(t => t.id === "shop");'
  + ' sel = {desk:0, grp:0, tab:i, global:false}; render(); await ' + wait(300) + ';'
  + ' document.getElementById("tab-words").scrollIntoView({block:"start"}); })()';
const pickers = 'document.querySelectorAll("#tab-words select")';
// The deciding AI's picker on the AI agents page: the one select of that card
// that is not the assistant AI's
const decider = 'document.querySelector("#ai-assistant select:not(#aiengine)")';

export default {
  config: {
    decide_ai: 'jev/jev-latest',
    providers: {
      jev: { base_url: 'https://api.typesafe.ai/v1/systemone', speaks: 'choice', models: ['jev-latest'] },
      deepseek: { base_url: 'https://api.deepseek.com', models: ['deepseek-flash'] },
    },
    agreed: { jev: ['pages'] },
    desks: [{
      name: 'Work', id: 'work',
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
    // The AI agents page: the deciding AI reads what the settings say, and
    // the providers list has the installed AIs first, each with its agreements
    agents: '(async () => { await ' + aiPage + ';'
      + ' const s = ' + decider + ';'
      + ' if (!s || s.value !== "jev") throw new Error("the deciding AI reads " + (s && s.value));'
      + ' const rows = [...document.querySelectorAll("#providerslist .secretrow")];'
      + ' if (!rows.length || !rows[0].textContent.includes("★")) throw new Error("the installed AIs are not first");'
      + ' const jev = rows.find(r => r.textContent.includes("jev"));'
      + ' if (!jev || !/1 \\/ 1/.test(jev.textContent)) throw new Error("jev\'s agreement is not on its row: " + (jev && jev.textContent)); })()',
    // A browser tab that says nothing of its own follows the app
    tab: tabPage,
    // A decision model where the writer goes: warned where it is, and refused
    wrong: '(async () => { await ' + tabPage + ';'
      + ' const s = ' + pickers + '[1];'
      + ' if ([...s.options].some(o => o.value === "jev")) throw new Error("a decision model is offered as the writer");'
      + ' const t = desks[0].tabs.find(t => t.id === "shop"); t.words_model = "jev/jev-latest"; render(); await ' + wait(200) + ';'
      + ' await save(); await ' + wait(300) + ';'
      + ' if (!document.querySelector("#tab-words .site-warn")) throw new Error("no warning under the writer");'
      + ' document.getElementById("tab-words").scrollIntoView({block:"start"}); })()',
    // The last line of the picker: a connection added there lands selected,
    // and the app takes it as its deciding AI when it had none
    added: '(async () => { await ' + tabPage + ';'
      + ' delete current.decide_ai;'
      + ' render(); await ' + wait(200) + '; document.getElementById("tab-words").scrollIntoView({block:"start"});'
      + ' const s = ' + pickers + '[0];'
      + ' s.value = "+add"; s.dispatchEvent(new Event("change")); await ' + wait(300) + ';'
      + ' const box = document.querySelector(".modal:last-of-type");'
      + ' const pre = box.querySelector(".mbody select");'
      + ' pre.value = [...pre.options].find(o => o.textContent.startsWith("OpenAI")).value; pre.dispatchEvent(new Event("change"));'
      + ' const model = [...box.querySelectorAll("input.mono")].find(i => i.style.flex); model.value = "gpt-6-sol";'
      + ' box.querySelector(".mfoot button.primary").click(); await ' + wait(400) + ';'
      + ' const t = desks[0].tabs.find(t => t.id === "shop");'
      + ' if (t.choose_model !== "openai/gpt-6-sol") throw new Error("not selected: " + t.choose_model);'
      + ' if (current.decide_ai !== "openai/gpt-6-sol") throw new Error("the app did not take it");'
      + ' document.getElementById("tab-words").scrollIntoView({block:"start"}); })()',
    // The AIs installed here head the list, marked, and each is chosen with
    // no model (its own) or with one of the names every account has
    subscription: '(async () => { await ' + aiPage + ';'
      + ' const s = ' + decider + ';'
      + ' const first = s.querySelector("optgroup");'
      + ' if (!first || !first.querySelector("option[value=\\"@claude\\"]")) throw new Error("the installed AIs are not first: " + (first && first.label));'
      + ' s.value = "@claude"; s.dispatchEvent(new Event("change")); await ' + wait(200) + ';'
      + ' if (current.decide_ai !== "@claude") throw new Error("written as " + current.decide_ai);'
      + ' const chip = [...document.querySelectorAll("#ai-assistant button")].find(b => b.textContent === "haiku");'
      + ' chip.click(); await ' + wait(200) + ';'
      + ' if (current.decide_ai !== "@claude/haiku") throw new Error("written as " + current.decide_ai);'
      + ' document.getElementById("ai-assistant").scrollIntoView({block:"start"}); })()',
    // A page that chose nothing, with no deciding AI chosen for the app, is
    // driven with the assistant AI, and says so; without a decision model it
    // is [Slow], and pressing that says why and lights the field that fixes it
    slow: '(async () => { delete current.decide_ai; await ' + tabPage + ';'
      + ' const s = ' + pickers + '[0];'
      + ' if (!s.options[0].textContent.includes("Claude Code")) throw new Error("the unset choice reads " + s.options[0].textContent);'
      + ' const tag = document.querySelector("#tab-words button.speedchip");'
      + ' if (!tag) throw new Error("no [Slow] to press");'
      + ' tag.click(); await ' + wait(150) + ';'
      + ' if (document.querySelector("#tab-words .site-warn").hidden) throw new Error("pressing [Slow] said nothing");'
      + ' if (!s.classList.contains("lookhere")) throw new Error("the field that fixes it did not light up");'
      + ' document.getElementById("tab-words").scrollIntoView({block:"start"}); })()',
    // A decision model deciding is [Fast], and there is nothing to press
    fast: '(async () => { await ' + tabPage + ';'
      + ' if (document.querySelector("#tab-words button.speedchip")) throw new Error("still slow with Jev deciding");'
      + ' if (!document.querySelector("#tab-words span.speedchip")) throw new Error("no [Fast]");'
      + ' document.getElementById("tab-words").scrollIntoView({block:"start"}); })()',
    // A connection's row opened: its fields, and under them what it may be
    // sent. Ticking "Send pages" is the same tick a refused goal offers
    agreements: '(async () => { await ' + aiPage + ';'
      + ' const row = [...document.querySelectorAll("#providerslist .secretrow")].find(r => r.textContent.includes("deepseek"));'
      + ' row.click(); await ' + wait(300) + ';'
      + ' const box = document.querySelector(".modal:last-of-type");'
      + ' const ticks = [...box.querySelectorAll(".consent input[type=checkbox]")];'
      + ' if (ticks.length !== 1) throw new Error("a connection is offered " + ticks.length + " kinds");'
      + ' ticks[0].click(); await ' + wait(100) + ';'
      + ' if (!(current.agreed.deepseek || []).includes("pages")) throw new Error("the tick was not written: " + JSON.stringify(current.agreed)); })()',
  },
  langs: ['ja', 'en'],
  sizes: [['wide', 1280, 900], ['phone', 390, 820]],
};
