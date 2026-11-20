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
            return {made, kept, fifty, more, fewer, button, diffMade, diffFifty, diffKept, diffMore};
          } finally { G = was.G; box.hidden = was.hidden; gitUi = null; box.textContent = ""; }
        })()`);
        if (git.diffMade !== 2000) fail(what, 'a 50000-line change drew ' + git.diffMade + ' lines at once, not 2000');
        if (!git.diffKept) fail(what, 'the change was drawn again for a state that changed nothing in it');
        if (git.diffFifty > 250) fail(what, 'fifty redraws of an unchanged change took ' + Math.round(git.diffFifty) + ' ms');
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
