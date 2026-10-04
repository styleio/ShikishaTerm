/**
 * The search bar over a page and the download list beside it, for
 * tools/debug/shoot.mjs. How a scene file is written is at the top of
 * files.mjs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/browser-find.mjs
 *     node tools/debug/shoot.mjs tools/debug/scenes/browser-find.mjs --remote
 *
 * The window's own page: the search row stands under the bar, the bar carries
 * the search and download buttons, a download beginning on the page in front
 * opens the list in the column, and a finished file at the PC is opened or
 * shown in its folder. `--remote` serves the phone's page instead, where the
 * list opens by the bar's button and a finished file is saved to the device
 * in hand.
 */

const remote = process.argv.includes('--remote');

const page = (index, name, extra) => Object.assign({
  index, name, id: name, state: 'WEB', state_label: 'Web', profile: '',
  locked: false, depth: 0, activity: [], group: 0, kind: 'browser',
  model: false, busy: false, settings: false, auto: false, restartable: true,
  readable: false, key: 'page:' + name,
}, extra);

const MB = 1024 * 1024;
const downloads = [
  { id: 'w5', name: 'catalogue-2026.pdf', site: 'shop.example.com', url: 'https://shop.example.com/files/catalogue-2026.pdf',
    path: '', got: 31.4 * MB, total: 48.2 * MB, state: 'going', why: '', page: 'shop', far: false, runs: false, began: 5 },
  { id: 'w4', name: 'prices.csv', site: 'shop.example.com', url: 'https://shop.example.com/export/prices.csv',
    path: 'C:\\Users\\me\\Downloads\\prices.csv', got: 182300, total: 182300, state: 'done', why: '', page: 'shop',
    far: false, runs: false, began: 4 },
  { id: 'w3', name: 'setup-tool.exe', site: 'tools.example.org', url: 'https://tools.example.org/setup-tool.exe',
    path: 'C:\\Users\\me\\Downloads\\setup-tool.exe', got: 9.1 * MB, total: 9.1 * MB, state: 'done', why: '', page: 'shop',
    far: false, runs: true, began: 3 },
  { id: 'w2', name: 'photos.zip', site: 'media.example.net', url: 'https://media.example.net/photos.zip',
    path: '', got: 3.2 * MB, total: 120 * MB, state: 'failed', why: 'network', page: 'shop', far: false, runs: false, began: 2 },
  { id: 'w1', name: 'manual-old.pdf', site: 'shop.example.com', url: 'https://shop.example.com/manual-old.pdf',
    path: '', got: 0, total: 0, state: 'cancelled', why: '', page: 'shop', far: false, runs: false, began: 1 },
];

const state = (extra) => JSON.stringify(JSON.stringify(Object.assign({
  desk: 'work', desk_id: 'work', desks: ['work'], desk_index: 0, active: 1,
  hotkeys: {}, quick: { cols: 0, rows: 0, pages: 0, items: [], dests: [] }, quick_to: {},
  groups: [{ name: 'site', folder: 'D:/work/site', color: '#4285f4', linked: false,
    health: { as: 'fine' }, drift: { behind: 0, ahead: 0 } }],
  tabs: [page(1, 'shop')],
  ball: { holder: 0, from: 0, depth: 0, max: 0, phase: '', progress: 0, awaiting_human: false },
  auto_enabled: true, remote_on: false, restartable: true, build: '',
  help_rows: [], ais: [],
  nav: { back: true, forward: true, reload: true, develop: true, edit: true, point: true, find: true,
    can_back: true, can_forward: false, at: 'https://shop.example.com/cart', loading: false },
  downloads: [], download_seq: 0,
}, extra)));

const fail = (why) => `throw new Error(${JSON.stringify(why)})`;

export default {
  served: remote ? 'remote' : 'window',
  settle: 1200,
  sizes: remote ? [['phone', 390, 820]] : [['window', 1280, 800], ['phone', 390, 820]],
  scenes: {
    // Ctrl+F on a page: the row under the bar, the words, where it stands,
    // and the page pushed down by both rows
    find: `window.__state(${state({ seek: { page: 'shop', text: '送料', at: 2, of: 7, asked: false, opened: 1 } })});
      new Promise(r => setTimeout(r, 300)).then(() => {
        const bar = document.getElementById("seek");
        if (!bar || bar.hidden) ${fail('the search row is not up')};
        if (bar.querySelector("input").value !== "送料") ${fail('the words are not in the box')};
        const count = bar.querySelector(".count").textContent;
        if (count !== (T["tui.seek.count"] || "").replace("{at}", 2).replace("{of}", 7)) ${fail('where the search stands is not said')};
        if (document.activeElement !== bar.querySelector("input")) ${fail('the cursor is not in the box once it opens')};
        const navh = getComputedStyle(document.getElementById("main")).getPropertyValue("--navh").trim();
        if (navh !== "72px") ${fail('the page is not pushed down by both rows')};
        const top = document.getElementById("seek").getBoundingClientRect().top - document.getElementById("nav").getBoundingClientRect().top;
        if (Math.round(top) !== 36) ${fail('the row is not right under the bar')};
        if (!document.querySelector("#nav button .ico svg")) ${fail('the bar has no search button')};
      })`,
    // Nothing found: said in words, not as a zero
    none: {
      run: `window.__state(${state({ seek: { page: 'shop', text: 'zzzz', at: 0, of: 0, asked: false, opened: 1 } })});
        new Promise(r => setTimeout(r, 300)).then(() => {
          if (document.querySelector("#seek .count").textContent !== T["tui.seek.none"]) ${fail('no match is not said as such')};
        })`,
      looks: ['dark'],
    },
    // A download begins on the page in front: at the window the list comes up
    // in the column; on a phone a line says so and the column stays away
    downloads: `window.__state(${state({ downloads: downloads.slice(1), download_seq: 4 })});
      window.__state(${state({ downloads, download_seq: 5 })});
      new Promise(r => setTimeout(r, 400)).then(() => {
        const panel = document.getElementById("dlpanel");
        const phone = phoneWidth();
        if (!phone && (!panel || panel.hidden)) ${fail('the list did not come up beside the page')};
        if (phone && !(panel && panel.hidden)) ${fail('the list covered the page on a phone')};
        if (phone) sideReveal("downloads");
        const rows = document.querySelectorAll("#dlpanel .dl");
        if (rows.length !== 5) ${fail('not every download has a row')};
        if (!rows[0].querySelector(".pbar")) ${fail('a download still coming has no bar')};
        if (!rows[3].querySelector(".ds.bad")) ${fail('why one stopped is not said')};
        const btn = document.querySelector("#nav .navdl");
        if (!btn || !btn.textContent.includes("65%")) ${fail('the download button does not say how far the newest has got')};
        const doneActs = rows[1].querySelectorAll(".da .pa").length;
        const exeActs = rows[2].querySelectorAll(".da .pa").length;
        if (AT_PC && doneActs !== 3) ${fail('a finished file at the PC is not offered open, folder and remove')};
        if (AT_PC && exeActs !== 2) ${fail('a program is offered to be opened')};
        if (!AT_PC && !rows[1].querySelector('a[download="prices.csv"]')) ${fail('a finished file is not offered to this device')};
        if (!AT_PC && rows[1].querySelector('a').getAttribute("href").includes("t=")) ${fail('the address of a file carries the key')};
      })`,
    // A page whose controls leave the search out: no button, and Ctrl+F on
    // the board is not taken (the browser's own box answers it in the page)
    unoffered: {
      run: `window.__state(${state({ nav: { back: true, forward: true, reload: true, develop: false, edit: true, point: false, find: false,
          can_back: false, can_forward: false, at: 'https://shop.example.com/', loading: false } })});
        new Promise(r => setTimeout(r, 300)).then(() => {
          if ([...document.querySelectorAll("#nav button")].some(b => b.title === T["tui.nav.find"])) ${fail('the search button is there though the page leaves it out')};
          const sent = []; const was = window.send; window.send = o => sent.push(o);
          const e = new KeyboardEvent("keydown", {key: "f", code: "KeyF", ctrlKey: true, bubbles: true, cancelable: true});
          document.body.dispatchEvent(e);
          window.send = was;
          if (sent.some(o => o.kind === "seek")) ${fail('Ctrl+F opened the search on a page that leaves it out')};
        })`,
      looks: ['dark'],
      sizes: [['window', 1280, 800]],
    },
    // Every line ended and gone: what appears here is said where the rows would be
    empty: {
      run: `window.__state(${state({ downloads: [], download_seq: 0 })}); sideReveal("downloads");
        new Promise(r => setTimeout(r, 300)).then(() => {
          if (!document.querySelector("#dlpanel .fsay")) ${fail('an empty list says nothing')};
        })`,
      looks: ['dark'],
    },
  },
};
