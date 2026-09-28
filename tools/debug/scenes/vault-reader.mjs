/**
 * Find, and one past conversation read whole in it, for tools/debug/shoot.mjs.
 *
 * The list, then a conversation opened from it: the word looked for marked
 * and in view, a long answer and a long block of code folded, the work
 * between a question and its answer one line (one of them opened), and
 * Resume -- plain where the folder is there, with its list of places where a
 * worktree was removed. Answered the way the app answers them, through
 * `window.__vaultRead`, rather than by reading records that are not here.
 */

const folder = 'D:/work/shop';
const hits = [
  {program: 'claude', id: 'aaaa', title: 'shop', cwd: folder, when: 1790000000,
    snippet: '…The cart total is computed by the pricing module on every request…'},
  {program: 'codex', id: 'bbbb', title: 'shop-fix-tax', cwd: 'D:/work/shop-fix-tax', when: 1789900000,
    snippet: 'Explain how the pricing rules apply tax to a discounted line…'},
];

const setup = `
  S = Object.assign(S || {}, {tabs: [], active: 0,
    groups: [{name: "shop", key: "${folder}", folder: "${folder}", project: "shop", color: "#5b7cff"}],
    vault: {query: "pricing", hits: ${JSON.stringify(hits)}, capped: false}});
  window.__openVault();
  const q = document.getElementById("vq"); q.value = "pricing";
  renderVault();
  "ok"`;

const explanation = Array.from({length: 24}, (_, i) =>
  `Step ${i + 1}: the rule reads the line's price, then its discount, then the tax band it falls in.`).join('\n');
const code = '```rust\n' + Array.from({length: 24}, (_, i) => `    let band_${i} = tax_band(line, ${i});`).join('\n') + '\n```';

const answer = (exists) => ({
  req: 1, ok: true, folder: exists ? folder : 'D:/work/shop-fix-tax', exists, branch: 'fix/tax',
  homes: exists ? [] : [{dir: folder, local: true}],
  items: [
    {k: 'say', who: 'you', text: 'the checkout page is slow'},
    {k: 'work', calls: 3, from: 100, to: 900, hit: true, work: {pieces: [
      {kind: 'say', text: 'Let me look at the logs first.'},
      {kind: 'call', name: 'Bash', text: 'tail -n 200 logs/app.log'},
      {kind: 'out', text: 'GET /checkout 200 812ms\nGET /checkout 200 794ms\nslow query in pricing.total (740ms)', hit: true, after: 18233},
    ]}},
    {k: 'say', who: 'ai', text: 'The cart total is computed by the **pricing** module on every request, and it reads every tax band from the database each time.\n\nI would cache the bands for a minute.', hit: true},
    {k: 'say', who: 'you', text: 'explain how the rules apply tax'},
    {k: 'work', calls: 12, from: 1000, to: 5000},
    {k: 'say', who: 'ai', text: explanation + '\n\n' + code},
    {k: 'say', who: 'you', text: 'cache it'},
    {k: 'work', calls: 4, from: 6000, to: 7000},
    {k: 'say', who: 'ai', text: 'Cached the tax bands for sixty seconds. The checkout page now answers in about 90 ms.'},
  ],
});

const open = (exists) => `
  openVaultRead(${JSON.stringify(hits[exists ? 0 : 1])}, "pricing");
  vaultReading.req = 1;
  window.__vaultRead(${JSON.stringify(answer(exists))});
  "ok"`;

export default {
  setup,
  scenes: {
    list: 'renderVault(); "ok"',
    reading: open(true),
    gone: {
      run: open(false) + '; document.querySelector("#vault .vrgo").click(); "ok"',
      looks: ['dark'],
    },
  },
  settle: 1200,
};
