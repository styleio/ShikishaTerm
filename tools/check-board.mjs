/**
 * Does the board come up at all? Asked of the page itself, outside the app.
 *
 *     node tools/check-board.mjs            the window's page and a phone's
 *     node tools/check-board.mjs --keep     leave the pages behind to look at
 *
 * Why it exists. The window shows a splash until the app hands the page its
 * first board state, and the page takes that state in one function
 * (`window.__state`). A throw anywhere in that function is invisible: the app
 * runs it through a call whose failure is reported back to the app and nobody
 * else, so the screen just keeps spinning and the log says nothing. A page that
 * cannot take its first state is an app that never starts -- and the way that
 * has happened is ordinary editing: a name declared inside that function which
 * the top of the same function already reads (`const holding` under a `holding`
 * from the page's own scope), a typo in a branch that only a certain state
 * reaches, a call to something that was renamed.
 *
 * So the page is written out as it is served (src/bin/page_dump.rs), opened in
 * a headless Chrome, handed the state the app hands it (the same `UiState` the
 * app sends, from the same binary), and asked two things: did anything throw,
 * and did the splash come down. Nothing here is a fixture: the state comes from
 * the app's own type, so a field added to it is in this check the day it lands.
 *
 * It needs Chrome (CHROME says where, if it is somewhere unusual) and cargo.
 * Nothing here ships; it runs in CI and on the machine of whoever is editing
 * the page.
 */
import {findChrome, findCargo, startChrome} from './debug/chrome.mjs';
import fs from 'node:fs';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

const ROOT = path.resolve(import.meta.dirname, '..');
const OUT = path.join(ROOT, 'target', 'board-check');
const KEEP = process.argv.includes('--keep');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(1); };

const cargo = findCargo();
/** Ask the app itself for something: the page as it serves it, or a state. */
function fromApp(args) {
  const made = spawnSync(cargo, ['run', '--quiet', '--bin', 'page_dump', '--', ...args],
    { cwd: ROOT, maxBuffer: 1 << 28 });
  if (made.status !== 0) die('page_dump ' + args.join(' ') + ' failed: ' + made.stderr);
  return made.stdout.toString('utf8');
}

/** A talking connection to one tab of a headless Chrome. */
const connect = chrome => startChrome({chrome});

fs.mkdirSync(OUT, { recursive: true });
// The state first: it is the same for every page, and asking the app for it
// once keeps a build out of the middle of the checks
const STATE = fromApp(['state']);
const chrome = await connect(findChrome());
await chrome.send('Page.enable');
await chrome.send('Runtime.enable');
// The page talks to the app through a bridge that is not here. A place that
// swallows what it is told, installed before the page's own script runs --
// that script gives up on its first throw, and everything below it with it
await chrome.send('Page.addScriptToEvaluateOnNewDocument',
  { source: 'window.ipc = { postMessage(){} };' });

let bad = 0;
const fail = (what, why) => { console.error('  FAILED ' + what + ': ' + why); bad += 1; };

// Both pages, in both languages: the window's, and the one a phone is served.
// The languages because the wording is baked into the page, and a page that
// only comes up in English is a page that does not come up here
for (const side of ['window', 'remote']) {
  for (const lang of ['en', 'ja']) {
    const what = side + '/' + lang;
    const file = path.join(OUT, 'page.' + lang + '.' + side + '.html');
    fs.writeFileSync(file, fromApp(side === 'remote' ? [lang, 'remote'] : [lang]));
    chrome.thrown.length = 0;
    await chrome.send('Emulation.setDeviceMetricsOverride',
      { width: side === 'remote' ? 390 : 1280, height: side === 'remote' ? 820 : 860,
        deviceScaleFactor: 1, mobile: side === 'remote' });
    await chrome.send('Page.navigate', { url: pathToFileURL(file).href });
    await sleep(900);
    // The page's own script has to have run to its end: it says it is up on
    // its last line, and everything the app then sends goes to what that
    // script defined. A page that stopped halfway still looks like a page
    const up = await chrome.run('typeof window.__state === "function" && typeof window.__screen === "function"');
    if (!up) fail(what, 'the page never finished starting (no __state)');
    if (chrome.thrown.length) fail(what, 'it threw while starting: ' + chrome.thrown.join(' | '));

    chrome.thrown.length = 0;
    // The first state, exactly as the app hands it over
    try {
      await chrome.run('window.__state(' + JSON.stringify(STATE) + ')');
    } catch (e) {
      fail(what, 'the first state threw: ' + String(e.message).split('\n')[0]);
    }
    await sleep(250);
    if (chrome.thrown.length) fail(what, 'the first state threw: ' + chrome.thrown.join(' | '));
    // What a person sees: the splash is the app saying "not yet", and it comes
    // down when, and only when, the board has been drawn
    const splash = await chrome.run('(document.getElementById("splash")||{}).hidden === true');
    if (!splash) fail(what, 'the splash stayed up after the board arrived');
    // The page binds a message to the tab in its view before either transport
    // sees it. An explicitly captured identity survives a later view change.
    try {
      // What goes over the wire leaves one request at a time, in order: the
      // sends are waited for, so that each has left -- bound to the tab it was
      // bound to when it was sent -- before the page's own fetch is put back
      const sent = await chrome.run(`(async () => {
        const state = S, ipc = window.ipc, fetch = window.fetch, out = [];
        try {
          S = {...S, active: 2, tabs: [{index: 2, uid: "original-tab"}]};
          window.ipc = {postMessage: text => out.push(JSON.parse(text))};
          window.fetch = (_, opts) => {out.push(JSON.parse(opts.body)); return Promise.resolve({});};
          const going = [send({kind: "say", tab: 2, text: "first"})];
          S.tabs = [{index: 2, uid: "replacement-tab"}];
          going.push(send({kind: "say", tab: 2, uid: "original-tab", text: "captured"}));
          going.push(send({kind: "say", tab: 9, text: "missing"}));
          await Promise.all(going);
          return out;
        } finally { S = state; window.ipc = ipc; window.fetch = fetch; }
      })()`);
      if (sent.length !== 2 || sent.some(s => s.uid !== 'original-tab')) fail(what, 'a message lost its recipient identity');
    } catch (e) { fail(what, 'sending threw: ' + String(e.message).split('\n')[0]); }
    // No access to this machine's real clipboard: simulate HTTP and a denied
    // permission, and check that copying falls back and pasting offers a field.
    if (side === 'remote') {
      try {
        const worked = await chrome.run(`(async () => {
          const descriptor = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
          const command = document.execCommand, copied = [];
          const body = el('div', {class:'sbody'}), field = el('input');
          const terminal = loginTerminal();
          body.append(terminal, el('div', {class:'lcoderow'}, field));
          document.body.append(body);
          try {
            document.execCommand = name => { if (name === 'copy') copied.push(document.activeElement.value); return true; };
            for (const clipboard of [undefined, {
              writeText: async () => { throw new Error('Permission denied'); },
              readText: async () => { throw new Error('Permission denied'); },
            }]) {
              Object.defineProperty(navigator, 'clipboard', {configurable:true, value:clipboard});
              field.focus();
              await copyToClipboard('Remote copy');
              if (document.activeElement !== field) return false;
              field.blur();
              terminal.dispatchEvent(new MouseEvent('contextmenu', {bubbles:true, cancelable:true}));
              await new Promise(r => setTimeout(r, 0));
              if (document.activeElement !== field) return false;
              if (document.getElementById('toastmsg').textContent !== T['tui.login.paste_here']) return false;
            }
            return copied.length === 2 && copied.every(text => text === 'Remote copy');
          } finally {
            if (descriptor) Object.defineProperty(navigator, 'clipboard', descriptor);
            else delete navigator.clipboard;
            document.execCommand = command; body.remove(); hideToast();
          }
        })()`);
        if (!worked) fail(what, 'clipboard fallback lost the copy or the paste field');
      } catch (e) { fail(what, 'clipboard fallback threw: ' + String(e.message).split('\n')[0]); }
    }
    // A folder whose build output git does not ignore: thousands of new files.
    // The git panel is drawn again on every state the app sends, and once made
    // a row for each of them every time -- the window stuttered for as long as
    // the panel was open. It makes the first ones, and makes them once
    if (side === 'window') {
      try {
        const git = await chrome.run(`(() => {
          const box = document.getElementById("gitpanel");
          const was = {G, hidden: box.hidden};
          try {
            box.hidden = false;
            G = gitFresh("many");
            G.rows = Array.from({length: 6000}, (_, i) => ({path: "out/f" + i + ".o", index: "?", work: "?",
              staged: false, unstaged: true, conflict: false, tangled: false}));
            G.branches = Array.from({length: 3}, (_, i) => ({name: "b" + i, current: i === 0}));
            drawGit();
            const list = gitUi.work, made = list.querySelectorAll(".row").length;
            const first = list.querySelector(".row");
            const t0 = performance.now();
            for (let i = 0; i < 50; i++) drawGit();
            const fifty = performance.now() - t0;
            const kept = list.querySelector(".row") === first;
            const offer = list.querySelector(".growsmore");
            if (offer) offer.click();
            const more = list.querySelectorAll(".row").length;
            G.rows = G.rows.slice(0, 10);
            drawGit();
            const fewer = list.querySelectorAll(".row").length, button = !!list.querySelector(".growsmore");
            // The change of a generated file: tens of thousands of lines,
            // drawn a part at a time and not looked at again while unchanged
            G.sel = G.rows[0].path; G.staged = false; G.diff = "diff --git a/x b/x";
            G.hunks = [{patch: "@@ -0,0 +1,50000 @@\\n" + Array.from({length: 50000}, (_, i) => "+line " + i).join("\\n"),
              start: 1, end: 50000, encoding: "", exact: true}];
            drawGit();
            const lines = () => gitUi.diff.querySelectorAll(".dlines > span").length;
            const diffMade = lines(), diffFirst = gitUi.diff.querySelector(".dlines > span");
            const d0 = performance.now();
            for (let i = 0; i < 50; i++) drawGit();
            const diffFifty = performance.now() - d0;
            const diffKept = gitUi.diff.querySelector(".dlines > span") === diffFirst;
            gitUi.diff.querySelector(".growsmore").click();
            const diffMore = lines();
            // A stage still going, and a reading asked before it answering
            // meanwhile: the button still says it is working. Then the stage
            // fails part of the way through, and the lists are read again
            const asked = [], ipc = window.ipc;
            window.ipc = {postMessage: text => asked.push(JSON.parse(text))};
            let busyKept, refreshed;
            const tabWas = gitTab;
            try {
              gitTab = () => tabWas() || {id: "many", name: "many"};
              // The panel is about that tab's folder, so reading it again keeps it
              G.where = gitWhere(gitTab());
              window.__git({act: "stage", busy: true, done: 800, of: 6000});
              window.__git({act: "branch", ok: true, read: true, data: null});
              busyKept = G.busy === "stage" && /800/.test(gitUi.main.textContent);
              // The stage done while an unstage still runs: still working
              window.__git({act: "stage", ok: true, data: null, still: "unstage"});
              busyKept = busyKept && G.busy === "unstage" && gitUi.main.disabled && !G.far;
              asked.length = 0;
              window.__git({act: "stage", ok: false, error: "failed part of the way"});
              refreshed = asked.some(a => a.kind === "gitpanel" ? a.act === "status" : JSON.stringify(a).includes('"status"'));
            } finally { window.ipc = ipc; gitTab = tabWas; }
            return {made, kept, fifty, more, fewer, button, diffMade, diffFifty, diffKept, diffMore, busyKept, refreshed};
          } finally { G = was.G; box.hidden = was.hidden; gitUi = null; box.textContent = ""; }
        })()`);
        if (git.diffMade !== 2000) fail(what, 'a 50000-line change drew ' + git.diffMade + ' lines at once, not 2000');
        if (!git.diffKept) fail(what, 'the change was drawn again for a state that changed nothing in it');
        if (git.diffFifty > 250) fail(what, 'fifty redraws of an unchanged change took ' + Math.round(git.diffFifty) + ' ms');
        if (!git.busyKept) fail(what, 'a reading answering during a stage ended the button\'s "working"');
        if (!git.refreshed) fail(what, 'a stage that failed part of the way through did not read the folder again');
        if (git.diffMore !== 4000) fail(what, '"show more" of the change left ' + git.diffMore + ' lines, not 4000');
        if (git.made !== 200) fail(what, 'the git panel made ' + git.made + ' rows of 6000 at once, not 200');
        if (!git.kept) fail(what, 'the git panel made its list again for a state that changed nothing in it');
        if (git.fifty > 250) fail(what, 'fifty redraws of an unchanged git panel took ' + Math.round(git.fifty) + ' ms');
        if (git.more !== 400) fail(what, '"show more" left ' + git.more + ' rows, not 400');
        if (git.fewer !== 10 || git.button) fail(what, 'a short list after a long one is not drawn whole');
      } catch (e) { fail(what, 'the git panel threw: ' + String(e.message).split('\n')[0]); }
      // The files panel: a name search over a large tree, and a folder of
      // thousands of entries, each shown a part at a time
      try {
        const files = await chrome.run(`(() => {
          const box = document.getElementById("filepanel");
          const was = {hidden: box.hidden, hits: FS.hits, q: FS.q, rows: FS.rows, mode: FS.mode};
          try {
            box.hidden = false;
            FS.mode = "name"; FS.q = "f"; FS.hits = Array.from({length: 6000}, (_, i) => ({path: "out/f" + i + ".o"})); FS.hitsUpTo = 0;
            FS.rev++; drawFiles();
            const hits = fsUi.list.querySelectorAll(".frow").length;
            fsUi.list.querySelector(".growsmore").click();
            const hitsMore = fsUi.list.querySelectorAll(".frow").length;
            FS.q = ""; FS.hits = null; FS.upTo = {};
            FS.rows = {"": Array.from({length: 6000}, (_, i) => ({name: "f" + i + ".o", dir: false}))};
            FS.rev++; drawFiles();
            const tree = fsUi.list.querySelectorAll(".frow").length;
            return {hits, hitsMore, tree};
          } finally { Object.assign(FS, was); FS.upTo = {}; FS.hitsUpTo = 0; box.hidden = was.hidden; fsUi = null; box.textContent = ""; }
        })()`);
        if (files.hits !== 200) fail(what, 'a search of 6000 hits made ' + files.hits + ' rows at once, not 200');
        if (files.hitsMore !== 400) fail(what, '"show more" of the hits left ' + files.hitsMore + ' rows, not 400');
        if (files.tree !== 200) fail(what, 'a folder of 6000 entries made ' + files.tree + ' rows at once, not 200');
      } catch (e) { fail(what, 'the files panel threw: ' + String(e.message).split('\n')[0]); }
      // Other long lists: a repository of thousands of branches, a Project
      // of a thousand cards, a folder of a thousand decisions -- each made a
      // part at a time, and made again only when it changed
      try {
        const lists = await chrome.run(`(() => {
          const out = {};
          // The branches, the one the folder is on far down the list
          const box = document.getElementById("gitpanel");
          const was = {G, hidden: box.hidden};
          try {
            box.hidden = false;
            G = gitFresh("branches");
            const names = Array.from({length: 3000}, (_, i) => "feature/" + String(i).padStart(4, "0"));
            G.branches = names.map((name, i) => ({name, current: i === 2900}));
            drawGit();
            out.branchRows = gitUi.branches.querySelectorAll(".row").length;
            out.hereShown = !!gitUi.branches.querySelector(".row.here");
            const first = gitUi.branches.querySelector(".row");
            // Asked again after a stage: a new answer naming the same branches
            G.branches = names.map((name, i) => ({name, current: i === 2900}));
            drawGit();
            out.branchesKept = gitUi.branches.querySelector(".row") === first;
            gitUi.branches.querySelector(".growsmore").click();
            out.branchMore = gitUi.branches.querySelectorAll(".row").length;
          } finally { G = was.G; box.hidden = was.hidden; gitUi = null; box.textContent = ""; }
          // A Project's board and table
          const ghWas = I.gh.upTo;
          try {
            I.gh.upTo = {};
            const status = {id: "f1", name: "Status", type: "SINGLE_SELECT", options: [{id: "o1", name: "Todo"}, {id: "o2", name: "Done"}]};
            const title = {id: "f0", name: "Title", type: "TITLE"};
            const d = {fields: [title, status], columns: "f1",
              cards: Array.from({length: 1000}, (_, i) => ({id: "c" + i, title: "card " + i, kind: "issue",
                values: {f1: {option: i < 900 ? "o1" : "o2"}}}))};
            const board = ghpBoard(d, {columns: "f1"});
            const cols = [...board.querySelectorAll(".ghcol")].map(c => c.querySelectorAll(".ghcard").length);
            out.boardCols = cols;
            out.boardCounts = [...board.querySelectorAll(".ghcol .ghhead .n")].map(n => n.textContent);
            board.querySelector(".growsmore").click();
            out.boardMore = I.gh.upTo;
            const table = ghpTable(d, {fields: ["f0", "f1"]});
            out.tableRows = table.querySelectorAll("tr[data-card]").length;
          } finally { I.gh.upTo = ghWas; }
          // The decisions, searched through their words
          const adWas = {records: AD.records, q: AD.q, status: AD.status, listEl: AD.listEl, footEl: AD.footEl, upTo: AD.upTo, upToFor: AD.upToFor};
          try {
            AD.listEl = el("div"); AD.footEl = el("div"); AD.status = "all"; AD.q = "";
            AD.records = Array.from({length: 1000}, (_, i) => ({number: i, file: i + ".md", title: "Decision " + i, status: "accepted",
              makers: "", text: (i % 3 ? "x " : "the ledger ") + "y".repeat(12000)}));
            adrFill();
            out.adrRows = AD.listEl.querySelectorAll(".arow").length;
            AD.q = "ledger";
            const t0 = performance.now();
            for (let i = 0; i < 20; i++) adrFill();
            out.adrTyped = performance.now() - t0;
            out.adrFound = AD.listEl.querySelectorAll(".arow").length + (AD.listEl.querySelector(".growsmore") ? "+" : "");
          } finally { Object.assign(AD, adWas); }
          return out;
        })()`);
        if (lists.branchRows !== 201) fail(what, 'a repository of 3000 branches made ' + lists.branchRows + ' rows at once, not 200 and the row for all of them');
        if (!lists.hereShown) fail(what, 'the branch the folder is on was not among the branches shown');
        if (!lists.branchesKept) fail(what, 'the same branches asked for again were made again');
        if (lists.branchMore !== 401) fail(what, '"show more" of the branches left ' + lists.branchMore + ' rows, not 400');
        if (JSON.stringify(lists.boardCols) !== JSON.stringify([0, 100, 100])) fail(what, 'the board made ' + JSON.stringify(lists.boardCols) + ' cards in its columns, not up to 100 each');
        if (JSON.stringify(lists.boardCounts) !== JSON.stringify(["0", "900", "100"])) fail(what, 'the columns counted ' + JSON.stringify(lists.boardCounts));
        if (!lists.boardMore || !Object.values(lists.boardMore).includes(200)) fail(what, '"show more" of a column did not ask for 200: ' + JSON.stringify(lists.boardMore));
        if (lists.tableRows !== 100) fail(what, 'the table made ' + lists.tableRows + ' rows of 1000 at once, not 100');
        if (lists.adrRows !== 200) fail(what, 'a thousand decisions made ' + lists.adrRows + ' rows at once, not 200');
        if (lists.adrFound !== "200+") fail(what, 'a search of the decisions showed ' + lists.adrFound + ', not the first 200 of 334');
        if (lists.adrTyped > 600) fail(what, "twenty letters typed in the decisions' search took " + Math.round(lists.adrTyped) + ' ms');
      } catch (e) { fail(what, 'the long lists threw: ' + String(e.message).split('\n')[0]); }
      // A terminal printing moves its bars five times a second. Only they
      // are handed over then, and only they are drawn again: the row they
      // are on stays the one that was made
      try {
        const pulse = await chrome.run(`(() => {
          // A terminal among the tabs: an AI's row says how long ago instead
          const was = JSON.stringify(S);
          try {
          S = {...S, active: 1, tabs: [{index: 1, name: "sh", id: "sh", key: "pulse-check", kind: "pty", state: "WAIT",
            state_label: "WAIT", profile: "GENERIC", activity: [0, 0, 0], cost: "1%"}]};
          drawTabs();
          const box = document.querySelector('.spark[data-spark^="t:"]');
          if (!box) return {none: true, tabs: S.tabs.map(t => [t.kind, t.ai, t.group]).slice(0, 5)};
          const key = box.dataset.spark.slice(2);
          const at = S.tabs.findIndex(t => t.key === key);
          const row = box.parentNode;
          const nav = document.getElementById("tabs") || document.body;
          const outside = () => [...nav.querySelectorAll("*")].filter(e => !e.closest(".spark"));
          const rows = outside();
          window.__pulse([{at, activity: [7, 7, 7, 7, 7, 7, 7, 7, 7, 7], cost: "9%"}]);
          const now = document.querySelector('.spark[data-spark="t:' + key + '"]');
          return {rowKept: !!now && now.parentNode === row, tall: now ? [...now.children].every(i => i.style.height === "14px") : false,
            inState: S.tabs[at].activity.join("") === "7777777777" && S.tabs[at].cost === "9%",
            rest: outside().every((e, i) => e === rows[i]) && outside().length === rows.length};
          } finally { S = JSON.parse(was); drawTabs(); }
        })()`);
        if (pulse.none) fail(what, 'no tab row drew its bars to be moved: ' + JSON.stringify(pulse.tabs));
        else {
          if (!pulse.rowKept) fail(what, 'the bars moving made the row again');
          if (!pulse.tall) fail(what, 'the bars handed over were not drawn');
          if (!pulse.inState) fail(what, 'the bars handed over did not reach the state the page keeps');
          if (!pulse.rest) fail(what, 'the bars moving changed the rest of the list');
        }
        // Under a press nothing is drawn; what moved meanwhile is drawn once it is over
        const held = await chrome.run(`(async () => {
          const was = JSON.stringify(S);
          try {
            const one = (key, n) => ({index: n, name: key, id: key, key, kind: "pty", state: "WAIT",
              state_label: "WAIT", profile: "GENERIC", activity: [0], cost: ""});
            S = {...S, active: 1, tabs: [one("held-a", 1), one("held-b", 2)]};
            drawTabs();
            holding = true;
            window.__pulse([{at: 0, key: "held-a", activity: [5, 5], cost: "3%"}]);
            const during = S.tabs[0].activity.join("");
            // The tabs moved about meanwhile: the bars still go to the tab they were for
            S.tabs = [S.tabs[1], S.tabs[0]];
            release();
            await new Promise(r => setTimeout(r, 20));
            const by = k => S.tabs.find(t => t.key === k).activity.join("");
            const other = by("held-b"), after = by("held-a");
            // Held again, and a newer state comes in the moment the press ends:
            // what was held is older than it and is dropped
            holding = true;
            window.__pulse([{at: 0, key: "held-b", activity: [1], cost: "5%"}]);
            release();
            const newer = JSON.parse(JSON.stringify(S));
            newer.tabs.find(t => t.key === "held-b").cost = "9%";
            window.__state(JSON.stringify(newer));
            await new Promise(r => setTimeout(r, 20));
            const kept = S.tabs.find(t => t.key === "held-b").cost;
            // ...and the same for bars that come in that moment
            holding = true;
            window.__pulse([{at: 0, key: "held-b", activity: [1], cost: "5%"}]);
            release();
            window.__pulse([{at: 0, key: "held-b", activity: [2], cost: "11%"}]);
            await new Promise(r => setTimeout(r, 20));
            const keptBars = S.tabs.find(t => t.key === "held-b").cost;
            return {during, after, other, kept, keptBars};
          } finally { holding = false; S = JSON.parse(was); drawTabs(); }
        })()`);
        if (held.during !== "0") fail(what, 'the bars were drawn under a press');
        if (held.after !== "55") fail(what, 'the bars that moved under a press were not drawn after it: ' + held.after);
        if (held.keptBars !== "11%") fail(what, "bars held over a press were laid over newer bars: " + held.keptBars);
        if (held.kept !== "9%") fail(what, "what was held over a press was laid over a newer state: " + held.kept);
        if (held.other !== "0") fail(what, 'the bars held over a press went to another tab once the tabs moved about');
      } catch (e) { fail(what, 'the bars moving threw: ' + String(e.message).split('\n')[0]); }
      // Removing a worktree that holds thousands of new files: the question
      // names the first of them, and the rest a press away
      try {
        const asked = await chrome.run(`(async () => {
          const files = Array.from({length: 500}, (_, i) => ({path: "out/f" + i + ".o", untracked: true, work: true}));
          let rows = null;
          await reviewFolderRemoval([{key: "k", folder: "wt"}], {
            request: async () => ({ok: true, files, keeps_branch: true, branch: "b", review: "r"}),
            question: async spec => { rows = spec.rows; return null; },
          });
          const box = el("div", {}, ...rows);
          document.body.append(box);
          try {
            const count = () => box.querySelectorAll(".brow2").length - 1;
            const first = count();
            box.querySelector("button").click();
            const more = count();
            box.querySelector("button").click();
            return {first, more, last: count(), button: !!box.querySelector("button")};
          } finally { box.remove(); }
        })()`);
        if (asked.first !== 200) fail(what, 'the removal question named ' + asked.first + ' files of 500 at once, not 200');
        if (asked.more !== 400 || asked.last !== 500 || asked.button) fail(what, 'the removal question did not show the rest: ' + JSON.stringify(asked));
      } catch (e) { fail(what, 'the removal question threw: ' + String(e.message).split('\n')[0]); }
    }
    // And the terminal's own contents, which arrive by their own call
    chrome.thrown.length = 0;
    try {
      await chrome.run('window.__screen("hello")');
    } catch (e) {
      fail(what, 'the screen threw: ' + String(e.message).split('\n')[0]);
    }
    if (chrome.thrown.length) fail(what, 'the screen threw: ' + chrome.thrown.join(' | '));
    if (!bad) console.log('  ok ' + what);
  }
}

chrome.stop();
// The pages, not the browser's own folder: Chrome is still letting go of its
// files as this runs, and a check that failed on tidying up would be a check
// that fails for no reason at all
if (!KEEP) {
  for (const f of fs.readdirSync(OUT)) {
    if (f.endsWith('.html')) { try { fs.rmSync(path.join(OUT, f)); } catch {} }
  }
}
if (bad) {
  console.error(bad + ' check(s) failed: the board does not come up');
  process.exit(1);
}
console.log('the board comes up');
