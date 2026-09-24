/**
 * 📼 over a browser tab, for tools/debug/shoot.mjs. How a scene file is
 * written is at the top of files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/words-panel.mjs
 *
 * 🗣 comes first and is what the panel opens on. A page whose models are not
 * chosen says so in the panel's line and has its own settings opened at the
 * two pickers -- once, not at every redraw -- and Send keeps the goal in the
 * box instead of sending it nowhere. A page with its models chosen shows the
 * ordinary line, with a ⚙ to its models either way.
 */

const page = (index, name, extra) => Object.assign({
  index, name, id: name, state: 'WEB', state_label: 'Web', profile: '',
  locked: false, depth: 0, activity: [], group: 0, kind: 'browser',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: 'page:' + name,
}, extra);

const state = (unset, fast) => JSON.stringify(JSON.stringify({
  desk: 'work', desk_id: 'work', desks: ['work'], desk_index: 0, active: 1,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [{ name: 'site', folder: 'D:/work/site', color: '#4285f4', linked: false,
    health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } }],
  tabs: [page(1, 'shop', { words_unset: unset, words_fast: !!fast })],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, remote_on: false, restartable: true, build: '',
  help_rows: [], ais: [],
}));

// The panel opened on 📼, with what the settings were asked for written down
// rather than opened (there is no app behind this page to open them)
const open = (unset, fast) => `window.__asked = [];
  openSettings = (...a) => { window.__asked.push(a); };
  window.__state(${state(unset, fast)});
  new Promise(r => setTimeout(() => {
    if (!castPanelEl) { r(Promise.reject(new Error("the bar never opened"))); return; }
    castPanel = "lua"; userPanel = "lua"; renderPanel(); renderPanel();
    const first = document.querySelector("#castlua input[type=radio]");
    if (!first || first.value !== "words" || !first.checked) {
      r(Promise.reject(new Error("the panel does not open on 🗣 first"))); return;
    }
    r("ok");
  }, 400))`;

export default {
  settle: 1200,
  sizes: [['window', 1280, 800], ['phone', 390, 820]],
  scenes: {
    // Models not chosen: the line says so, and the page's settings were asked
    // for once, at its models
    unset: `${open(true)}.then(() => {
      if (window.__asked.length !== 1) throw new Error("the settings were asked for " + window.__asked.length + " times");
      const [section, , , tab] = window.__asked[0];
      if (section !== "words" || !tab || tab.id !== "shop") throw new Error("asked for the wrong place: " + JSON.stringify(window.__asked[0]));
      castInput.value = "find the cheapest one"; sendBar();
      if (castInput.value !== "find the cheapest one") throw new Error("the goal was thrown away");
      if (window.__asked.length !== 2) throw new Error("Send did not ask for the models");
    })`,
    // Models chosen: the ordinary line, and nothing opened
    ready: `${open(false)}.then(() => {
      if (window.__asked.length) throw new Error("the settings opened though the models are chosen");
      // Nothing to say, so the panel says nothing: what each mode does is on
      // the mode, under the pointer
      if (document.querySelector("#castpanel .castnote")) throw new Error("the panel still carries a line of explanation");
      if (!document.querySelector("#castlua label.castradio[title]").title) throw new Error("the mode does not say what it does");
    })`,
    // No decision model picks the moves: [Slow] left of the gear, and
    // pressing it opens this page's settings at why
    slow: `${open(false)}.then(() => {
      const tag = document.querySelector("button.castspeed");
      if (!tag) throw new Error("no [Slow] beside the gear");
      const gear = document.querySelector(".castgear");
      if (tag.nextElementSibling !== gear) throw new Error("[Slow] is not just left of the gear");
      tag.click();
      const [section, , , tab] = window.__asked[window.__asked.length - 1] || [];
      if (section !== "words-slow" || !tab || tab.id !== "shop") throw new Error("pressed, it asked for " + JSON.stringify(window.__asked));
    })`,
    // A decision model picks them: [Fast], which is only a tag
    fast: `${open(false, true)}.then(() => {
      if (document.querySelector("button.castspeed")) throw new Error("[Fast] can be pressed");
      const tag = document.querySelector("span.castspeed");
      if (!tag || tag.nextElementSibling !== document.querySelector(".castgear")) throw new Error("no [Fast] just left of the gear");
    })`,
    // Sent, and refused for want of an agreement to send the page: the
    // reason and the button that agrees, on 🗣's own line. Pressing it sends
    // the same goal again, agreed
    agree: `${open(false)}.then(() => {
      const sent = []; window.ipc = { postMessage: (m) => sent.push(JSON.parse(m)) };
      window.__wordsNote({ text: "ページに出ている内容と、そこで行える操作の一覧を Claude Code に送ります。{by} を提供する会社のサービスに届き、その会社の規約に沿って扱われます。送って実行するには「同意して実行」を押してください。".replace("{by}", "Claude Code"), bad: false, agree: "find the cheapest one" });
      const b = document.querySelector(".castagree");
      if (!b) throw new Error("no button that agrees");
      return new Promise(r => setTimeout(r, 300)).then(() => {
        b.click();
        const m = sent.find(x => x.kind === "words");
        if (!m || !m.agree || m.goal !== "find the cheapest one") throw new Error("the press sent " + JSON.stringify(sent));
        if (document.querySelector(".castagree")) throw new Error("the button stayed after it was pressed");
        window.__wordsNote({ text: "ページに出ている内容と、そこで行える操作の一覧を Claude Code に送ります。Claude Code を提供する会社のサービスに届き、その会社の規約に沿って扱われます。送って実行するには「同意して実行」を押してください。", bad: false, agree: "find the cheapest one" });
      });
    })`,
  },
};
