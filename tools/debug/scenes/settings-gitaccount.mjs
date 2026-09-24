/**
 * Where a personal access token is put, for tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-gitaccount.mjs
 *
 * An app with no git account at all and one project -- the state somebody is
 * in when they arrive holding a token. The scenes walk the way through it: the
 * picker on the project's page, the desk's accounts opened over that page, the
 * token being typed, and the account chosen once the window is closed.
 *
 * The project is this checkout, because the account card only appears for a
 * folder the app answers about as a repository.
 */
import path from 'node:path';

const REPO = path.resolve(import.meta.dirname, '..', '..', '..').replaceAll('\\', '/');

const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';
// The project's page, waited for: the account card appears once the app has
// answered what repository the folder belongs to
const projectPage = '(async () => { sel = {desk:0, proj:"p:ShikishaTerm", grp:null, tab:null, global:false};'
  + ' render(); await ' + wait(900) + ';'
  + ' const c = document.getElementById("project-gitacct");'
  + ' if (!c) throw new Error("the account card is not on the project page");'
  + ' c.scrollIntoView({block:"center"}); })()';
// ...and the last line of its picker chosen, which opens the desk's accounts
const openWindow = '(async () => { await ' + projectPage + ';'
  + ' const s = document.querySelector("#project-gitacct select");'
  + ' s.value = "@add"; s.dispatchEvent(new Event("change")); await ' + wait(300) + '; })()';
// ...and "+ Add git account" pressed in it
const openDialog = '(async () => { await ' + openWindow + ';'
  + ' [...document.querySelectorAll(".modal .mbody button")].pop().click();'
  + ' await ' + wait(400) + ';'
  + ' document.querySelector(".modal:last-of-type input[type=password]").closest(".field").scrollIntoView({block:"center"}); })()';

export default {
  config: {
    desks: [{
      name: 'Work', id: 'work',
      projects: [{ name: 'ShikishaTerm', at: REPO }],
      folders: [{
        name: 'ShikishaTerm', cwd: REPO, project: 'ShikishaTerm',
        tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }],
      }],
    }],
  },
  scenes: {
    // Settings > Git accounts: the app's accounts, then the sign-ins this
    // PC's git and gh hold
    accounts: '(async () => { sel = {desk:0, tab:null, global:true}; goSection("gitaccounts", "center");'
      + ' await ' + wait(600) + '; window.scrollTo(0, 0); })()',
    // The desk's git page: what git does on this desk, with no accounts on it
    desk: '(async () => { sel = {desk:0, tab:null, global:true}; goDeskSection("git");'
      + ' await ' + wait(600) + '; window.scrollTo(0, 0); })()',
    // Nothing chosen and nothing to choose: the picker's own way out
    picker: projectPage,
    // The desk's accounts, over the page that asked for them
    window: openWindow,
    // The token being asked for, with what becomes of it said underneath
    dialog: openDialog,
    // The whole way through: an account made in the window, the window closed,
    // and the picker showing it chosen
    chosen: '(async () => { await ' + openDialog + ';'
      + ' const box = document.querySelector(".modal:last-of-type");'
      + ' const name = box.querySelector("input[type=text]");'
      + ' name.value = "work"; name.dispatchEvent(new Event("input"));'
      + ' const tok = box.querySelector("input[type=password]");'
      + ' tok.value = "github_pat_madeup"; tok.dispatchEvent(new Event("input"));'
      + ' box.querySelector(".mfoot button.primary").click(); await ' + wait(600) + ';'
      + ' const win = document.querySelector(".modal:last-of-type");'
      + ' win.querySelector(".mfoot button.primary").click(); await ' + wait(500) + ';'
      + ' const c = document.getElementById("project-gitacct");'
      + ' if (!c) throw new Error("the project page did not come back");'
      + ' c.scrollIntoView({block:"center"}); })()',
  },
  langs: ['ja', 'en'],
  sizes: [['wide', 1280, 900], ['phone', 390, 820]],
};
