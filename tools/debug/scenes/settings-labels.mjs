/**
 * Where a working folder's automatic name and summary are set, for
 * tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-labels.mjs
 *
 * A desk with a model connection, and three folders: one whose name and
 * summary were written for it (Auto on), one named by a person, and one just
 * made under Auto with nothing written yet. What is judged is whether Name,
 * Summary and Auto read as one question each in that order, and whether the
 * desk's choice of AI says what it costs without talking about tokens.
 */

const wait = (ms) => 'new Promise(r => setTimeout(r, ' + ms + '))';
const folderPage = (i) => '(async () => { sel = {desk:0, grp:' + i + ', tab:null, global:false}; render();'
  + ' await ' + wait(400) + '; window.scrollTo(0, 0); })()';

export default {
  config: {
    desks: [{
      name: 'site', id: 'site',
      providers: { deepseek: { base_url: 'https://api.deepseek.com/v1' } },
      folders: [
        { name: 'site', cwd: 'D:/work/site', tabs: [{ name: 'shell', command: 'cmd' }] },
        { name: 'Sign-in error messages', cwd: 'D:/work/site-login', auto_label: true,
          summary: 'Show why signing in failed -- wrong password, locked account or expired link -- instead of the blank form that comes back now.',
          tabs: [{ name: 'claude', command: 'claude' }] },
        { name: 'Pricing page', cwd: 'D:/work/site-pricing', tabs: [{ name: 'codex', command: 'codex' }] },
      ],
    }],
  },
  scenes: {
    // Written for it: Auto on
    auto: folderPage(1),
    // Named by a person
    named: folderPage(2),
    // Typing a name of one's own takes Auto off
    typing: '(async () => { await ' + folderPage(1) + ';'
      + ' const i = document.querySelector(".card input.grow, .card input[type=text]");'
      + ' i.value = "My own name"; i.dispatchEvent(new Event("input")); })()',
    // The desk's choice of AI, on a model connection
    desk: '(async () => { sel = {desk:0, grp:null, tab:null, global:false, dsection:"labels"}; render();'
      + ' await ' + wait(300) + '; const s = document.querySelector(".card select");'
      + ' s.value = "model:deepseek"; s.dispatchEvent(new Event("change")); })()',
  },
};
