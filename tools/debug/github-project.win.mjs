/**
 * A GitHub Project on the Issue tab, through the running app and GitHub itself.
 *
 * A folder whose origin is GHP_REPO (owner/name) is made a project of a copy
 * of this checkout's build, signed in as GHP_ACCOUNT: "pc" for the account
 * git on this PC holds, else the token GITHUB_PAT in .private/.env (GHP_PAT_VAR
 * names another variable there to take it from). The Issue
 * tab is opened and switched to Project. ("pc:<login>" picks one of several
 * accounts git on this PC holds.) Then, by what the token may do:
 *
 *   - it may not see GitHub Projects: the tab says so, with the way to the
 *     account's settings (this is checked whenever it is the answer)
 *   - it may: the GitHub Projects of the repository are listed; the first
 *     (or GHP_PICK, an address) is opened, and its view is drawn -- a board
 *     in columns, or a table -- with as many cards as GitHub says it has
 *
 * Only with GHP_WRITE=1 is anything written, and only to the GitHub Project
 * opened: the first card is moved to another column, read back from GitHub
 * there, and moved back. Point it at a GitHub Project of your own.
 *
 *     cargo build
 *     set GHP_REPO=owner/name
 *     node tools/debug/github-project.win.mjs
 *
 * Nothing of a copy somebody is using is read, written or stopped; the copy
 * is stopped on the way out (GHP_KEEP=1 leaves it running to be looked at).
 */
import {connectCdp} from './chrome.mjs';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const RUN = path.join(os.tmpdir(), 'sk-gh-project');
const APP = path.join(RUN, 'app');
const LOCAL = path.join(RUN, 'local');
const DIR = path.join(RUN, 'repo');
const SHOTS = path.join(ROOT, 'target', 'shots', 'github-project');
const REPO = process.env.GHP_REPO || '';
const ACCOUNT = process.env.GHP_ACCOUNT || 'token';
const WRITE = process.env.GHP_WRITE === '1';
const VIEWS = process.env.GHP_VIEWS === '1';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(2); };
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };
const ps = (...args) => spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', ...args], { encoding: 'utf8' });
const stopApp = () => ps('-Command',
  `Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'SHIKISHA-TERM.exe' -or $_.Name -eq 'msedgewebview2.exe') -and ($_.CommandLine + '') -like '*sk-gh-project*' } | ` +
  `ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`);
const lines = (text) => text.split(/\r?\n/);
if (!/^[\w.-]+\/[\w.-]+$/.test(REPO)) die('set GHP_REPO to owner/name');
const envFile = process.env.PRIVATE_ENV || path.join(ROOT, '.private', '.env');
const dotenv = fs.existsSync(envFile) ? Object.fromEntries(lines(fs.readFileSync(envFile, 'utf8'))
  .map((l) => l.match(/^([A-Z0-9_]+)=(.*)$/)).filter(Boolean).map((m) => [m[1], m[2].replace(/^"|"$/g, '')])) : {};
const PAT_VAR = process.env.GHP_PAT_VAR || 'GITHUB_PAT';
const PAT = dotenv[PAT_VAR];
if (!ACCOUNT.startsWith('pc') && !PAT) die(PAT_VAR + ' is needed in .private/.env (or GHP_ACCOUNT=pc)');
const exe = path.join(ROOT, 'target', 'debug', 'SHIKISHA-TERM.exe');
if (!fs.existsSync(exe)) die('no build at target\\debug -- run cargo build first');

stopApp();
await sleep(800);
fs.rmSync(RUN, { recursive: true, force: true });
for (const d of [APP, LOCAL, DIR, SHOTS, path.join(APP, 'config')]) fs.mkdirSync(d, { recursive: true });
const git = (...a) => { const r = spawnSync('git', ['-C', DIR, ...a], { encoding: 'utf8' }); if (r.status !== 0) die('git: ' + r.stderr); };
git('init', '-q', '-b', 'main');
git('remote', 'add', 'origin', `https://github.com/${REPO}.git`);
const staged = ps('-File', path.join(ROOT, 'tools', 'stage.ps1'), '-Dest', APP, '-Package', '-Exe', exe);
if (!fs.existsSync(path.join(APP, 'SHIKISHA-TERM.exe'))) die('staging failed:\n' + staged.stdout + staged.stderr);
const onPc = ACCOUNT === 'pc' || ACCOUNT.startsWith('pc:');
const account = ACCOUNT === 'pc' ? '@pc' : onPc ? '@' + ACCOUNT : 'check';
fs.writeFileSync(path.join(APP, 'config', 'config.json'), JSON.stringify({
  language: process.env.GHP_LANG || 'ja', remote: { enabled: false },
  agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' }, ai_engine: 'nobody-here',
  git_accounts: onPc ? [] : [{ name: 'check', owners: [REPO.split('/')[0]] }],
  desks: [{ name: 'Check', id: 'check',
    projects: [{ name: 'repo', at: DIR, git_account: account }],
    folders: [{ cwd: DIR, tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }] }],
}, null, 2));
if (!onPc) fs.writeFileSync(path.join(APP, 'config', 'secrets.json'), JSON.stringify({ tokens: { 'git/check': PAT } }, null, 2));

const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/i.test(k)));
env.LOCALAPPDATA = LOCAL;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=0';
const until = async (test, what, ms = 30000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await Promise.resolve().then(test).catch(() => false)) return true; await sleep(250); }
  throw new Error('timed out waiting for ' + what);
};
const portOf = () => {
  const f = path.join(LOCAL, 'ShikishaTerm', 'webview2', 'shell', 'EBWebView', 'DevToolsActivePort');
  if (!fs.existsSync(f)) return null;
  const n = Number(lines(fs.readFileSync(f, 'utf8'))[0]);
  return Number.isFinite(n) && n > 0 ? n : null;
};

// What GitHub itself says a view holds: its cards (the view's filter applied
// by GitHub) and the name of each card's value in every field
const truth = async (owner, org, number, view) => {
  const field = '... on ProjectV2FieldCommon { name }';
  const q = `query($o: String!, $n: Int!, $v: Int!, $q: String) { ${org ? 'organization' : 'user'}(login: $o) { projectV2(number: $n) {
    view(number: $v) { filter }
    items(first: 100, query: $q) { nodes { content { ... on DraftIssue { title assignees(first: 5) { nodes { login } } } ... on Issue { title assignees(first: 5) { nodes { login } } } ... on PullRequest { title assignees(first: 5) { nodes { login } } } }
      fieldValues(first: 50) { nodes {
        ... on ProjectV2ItemFieldSingleSelectValue { name field { ${field} } }
        ... on ProjectV2ItemFieldIterationValue { title field { ${field} } }
        ... on ProjectV2ItemFieldNumberValue { number field { ${field} } }
        ... on ProjectV2ItemFieldDateValue { date field { ${field} } }
        ... on ProjectV2ItemFieldTextValue { text field { ${field} } } } } } } } } }`;
  const ask = async (variables) => {
    const r = await (await fetch('https://api.github.com/graphql', { method: 'POST', headers: { Authorization: 'bearer ' + PAT },
      body: JSON.stringify({ query: q, variables }) })).json();
    if (r.errors) throw new Error(JSON.stringify(r.errors));
    return (r.data.organization || r.data.user).projectV2;
  };
  const filter = (await ask({ o: owner, n: number, v: view })).view.filter || null;
  const p = await ask({ o: owner, n: number, v: view, q: filter });
  return p.items.nodes.map((i) => ({ title: (i.content || {}).title || '',
    people: (((i.content || {}).assignees || {}).nodes || []).map((a) => a.login).join(', '), values: Object.fromEntries(i.fieldValues.nodes
    .filter((v) => v.field).map((v) => [v.field.name, v.name ?? v.title ?? v.number ?? v.date ?? v.text])) }));
};

// Every view of the GitHub Project opened, each drawn as GitHub lays it out:
// the cards GitHub's filter leaves, a board's columns in the field's order with
// each card in the column of its value, a table's groups and its order
async function everyView(run, shot, views) {
  const at = JSON.parse(await run('JSON.stringify({owner: I.gh.data.owner, org: I.gh.data.org, number: I.gh.data.number, none: T["ghp.no_value"] || ""})'));
  T_NONE = (field) => at.none.replaceAll('{field}', field);
  for (const [n, layout] of views) {
    await run(`ghpView(${n}); true`);
    await until(() => run(`!!I.gh.data && I.gh.data.view === ${n} && !I.gh.sending.length`), 'view ' + n, 60000);
    await sleep(300);
    const v = JSON.parse(await run(`JSON.stringify((() => { const d = I.gh.data, w = d.views.find(x => x.number === ${n}), f = id => (d.fields.find(x => x.id === id) || {});
      return {name: w.name, layout: w.layout, group: w.group && f(w.group).name, lanes: d.columns && f(d.columns).name, sort: (w.sort || []).map(s => [f(s.field).name, f(s.field).type, s.desc]),
        options: d.columns ? (f(d.columns).options || f(d.columns).iterations || []).map(o => o.name || o.title) : [],
        none: (T["ghp.no_value"] || "").replaceAll("{field}", d.columns ? f(d.columns).name : "") }; })())`));
    const real = await truth(at.owner, at.org, at.number, n);
    console.log(`   view ${n} "${v.name}" (${layout}): GitHub has ${real.length} cards` + (v.group ? `, grouped by ${v.group}` : '') + (v.sort.length ? `, sorted by ${v.sort.map((s) => s[0] + (s[2] ? ' desc' : '')).join(', ')}` : ''));
    await shot('view-' + n);
    const titles = real.map((c) => c.title).sort();
    if (layout === 'board') {
      const drawn = JSON.parse(await run(`JSON.stringify([...document.querySelectorAll("#issuespanel .ghlane")].length
        ? [...document.querySelectorAll("#issuespanel .ghlane")].map(l => ({lane: l.querySelector(".ghlanehead .grow").textContent, cols: [...l.querySelectorAll(".ghcol")].map(c => ({name: c.querySelector(".ghhead .grow").textContent, cards: [...c.querySelectorAll(".ghcard .t")].map(t => t.textContent)}))}))
        : [{lane: null, cols: [...document.querySelectorAll("#issuespanel .ghcol")].map(c => ({name: c.querySelector(".ghhead .grow").textContent, cards: [...c.querySelectorAll(".ghcard .t")].map(t => t.textContent)}))}])`));
      const all = drawn.flatMap((l) => l.cols.flatMap((c) => c.cards)).sort();
      check(JSON.stringify(all) === JSON.stringify(titles), `view ${n}: the cards are the ones GitHub's filter leaves (${all.length})`);
      let placed = true;
      for (const l of drawn) for (const c of l.cols) for (const t of c.cards) {
        const card = real.find((x) => x.title === t) || { values: {} };
        const want = card.values[v.lanes] ?? v.none;
        if (want !== c.name) { placed = false; console.log(`     "${t}" is under ${c.name}, GitHub has ${want}`); }
        if (l.lane !== null) {
          const lane = card.values[v.group] ?? (T_NONE(v.group));
          if (lane !== l.lane) { placed = false; console.log(`     "${t}" is in the lane ${l.lane}, GitHub has ${lane}`); }
        }
      }
      check(placed, `view ${n}: each card is in the column${v.group ? ' and the lane' : ''} of its ${v.lanes}${v.group ? ' and ' + v.group : ''}`);
      const names = drawn[0].cols.map((c) => c.name).filter((x) => x !== v.none);
      check(JSON.stringify(names) === JSON.stringify(v.options.filter((o) => names.includes(o))) && drawn[0].cols[0].name === v.none,
        `view ${n}: the columns are ${drawn[0].cols.map((c) => c.name).join(' | ')} (the cards with none first, then the field's order)`);
      if (v.group) check(drawn.length > 1, `view ${n}: the board is split into lanes by ${v.group} (${drawn.map((l) => l.lane).join(' | ')})`);
    } else {
      const heads = JSON.parse(await run(`JSON.stringify([...document.querySelectorAll("#issuespanel .ght thead th")].map(h => h.textContent))`));
      const who = heads.indexOf('Assignees');
      const drawn = JSON.parse(await run(`JSON.stringify([...document.querySelectorAll("#issuespanel .ght tbody tr")].map(r => r.classList.contains("grp") ? {group: r.firstChild.firstChild.textContent.trim()} : {title: r.querySelector(".ttl").textContent, people: ${who} < 0 ? null : r.children[${who}].textContent}))`));
      if (who >= 0) {
        const wrong = drawn.filter((r) => r.title !== undefined && r.people !== real.find((c) => c.title === r.title).people);
        for (const r of wrong) console.log(`     "${r.title}" shows "${r.people}" as its assignees, GitHub has "${real.find((c) => c.title === r.title).people}"`);
        check(!wrong.length, `view ${n}: each row shows the people GitHub has on the card`);
      }
      if (layout === 'roadmap') check(['Start', 'Due', 'Sprint'].every((h) => heads.includes(h)), `view ${n}: the roadmap, shown as a table, has its dates and iterations: ${heads.join(' | ')}`);
      const rows = drawn.filter((r) => r.title !== undefined).map((r) => r.title);
      check(JSON.stringify(rows.slice().sort()) === JSON.stringify(titles), `view ${n}: the rows are the cards GitHub's filter leaves (${rows.length})`);
      if (v.group) {
        const groups = drawn.filter((r) => r.group !== undefined).map((r) => r.group);
        let ok = true, lane = null;
        for (const r of drawn) {
          if (r.group !== undefined) { lane = r.group; continue; }
          const want = real.find((x) => x.title === r.title).values[v.group] ?? T_NONE(v.group);
          if (want !== lane) { ok = false; console.log(`     "${r.title}" is under ${lane}, GitHub has ${want}`); }
        }
        check(ok, `view ${n}: the rows are grouped by ${v.group}: ${groups.join(' | ')}`);
      }
      for (const [name, type, desc] of v.sort) {
        let ok = true, lane = null, last;
        for (const r of drawn) {
          if (r.group !== undefined) { lane = r.group; last = undefined; continue; }
          const x = real.find((c) => c.title === r.title).values[name];
          if (x === undefined) { last = null; continue; }
          if (last === null) ok = false;
          else if (last !== undefined && (desc ? x > last : x < last)) ok = false;
          last = x;
        }
        check(ok, `view ${n}: the rows are in ${name} order${desc ? ', highest first' : ''}, the ones with none last` + (type ? '' : ''));
      }
    }
  }
}
let T_NONE = () => '';

// One card as GitHub has it now: the name of its value in every field
const cardOnGithub = async (id) => {
  const field = '... on ProjectV2FieldCommon { name }';
  const r = await (await fetch('https://api.github.com/graphql', { method: 'POST', headers: { Authorization: 'bearer ' + PAT },
    body: JSON.stringify({ query: `query($id: ID!) { node(id: $id) { ... on ProjectV2Item { fieldValues(first: 50) { nodes {
      ... on ProjectV2ItemFieldSingleSelectValue { name field { ${field} } }
      ... on ProjectV2ItemFieldIterationValue { title field { ${field} } }
      ... on ProjectV2ItemFieldNumberValue { number field { ${field} } }
      ... on ProjectV2ItemFieldDateValue { date field { ${field} } }
      ... on ProjectV2ItemFieldTextValue { text field { ${field} } } } } } } }`, variables: { id } }) })).json();
  if (r.errors) throw new Error(JSON.stringify(r.errors));
  return Object.fromEntries(r.data.node.fieldValues.nodes.filter((v) => v.field)
    .map((v) => [v.field.name, v.name ?? v.title ?? v.number ?? v.date ?? v.text]));
};
const settled = (run) => until(() => run('!I.gh.sending.length && !I.bad'), 'the changes to reach GitHub', 60000);
const githubHas = async (id, name, want) => {
  const end = Date.now() + 30000;
  let now;
  while (Date.now() < end) { now = (await cardOnGithub(id))[name]; if (now === want) return true; await sleep(1000); }
  console.log(`     GitHub has ${name} = ${JSON.stringify(now)}, not ${JSON.stringify(want)}`);
  return false;
};

// Each kind of field a card's value can be changed in, as a table's cell
// changes it: put to another value, read back from GitHub, taken off, read
// back, and put back as it was
async function everyField(run) {
  console.log('4. each kind of field changed in place, read back, taken off, and put back');
  const plan = JSON.parse(await run(`JSON.stringify((() => { const d = I.gh.data, c = d.cards.find(x => x.kind === "draft") || d.cards.find(x => x.kind !== "hidden");
    return {card: c.id, title: c.title, fields: d.fields.filter(f => ["NUMBER", "DATE", "TEXT", "SINGLE_SELECT", "ITERATION"].includes(f.type)).map(f => {
      const v = c.values[f.id] || null, choices = f.options || f.iterations || [];
      const other = choices.find(o => o.id !== (v && (v.option || v.iteration)));
      const to = f.type === "NUMBER" ? {number: (v && v.number || 0) + 7} : f.type === "DATE" ? {date: v && v.date === "2026-12-24" ? "2026-12-25" : "2026-12-24"}
        : f.type === "TEXT" ? {text: "Changed from the Issue tab"} : f.type === "SINGLE_SELECT" ? {option: other.id} : {iteration: other.id};
      const name = x => x === null ? undefined : x.option ? choices.find(o => o.id === x.option).name : x.iteration ? choices.find(o => o.id === x.iteration).title
        : x.number !== undefined ? x.number : x.date !== undefined ? x.date : x.text;
      const back = v === null ? null : v.option ? {option: v.option} : v.iteration ? {iteration: v.iteration} : v.number !== undefined ? {number: v.number} : v.date ? {date: v.date} : {text: v.text};
      return {id: f.id, name: f.name, type: f.type, to, toName: name(to), back, backName: name(back)}; })}; })())`));
  const put = (f, value) => run(`(() => { const d = I.gh.data; ghpSet(d.cards.find(x => x.id === ${JSON.stringify(plan.card)}), d.fields.find(x => x.id === ${JSON.stringify(f.id)}), ${JSON.stringify(value)}); return true; })()`);
  for (const f of plan.fields) {
    await put(f, f.to); await settled(run);
    const set = await githubHas(plan.card, f.name, f.toName);
    await put(f, null); await settled(run);
    const off = await githubHas(plan.card, f.name, undefined);
    if (f.back !== null) { await put(f, f.back); await settled(run); }
    const back = await githubHas(plan.card, f.name, f.backName);
    check(set && off && back, `"${plan.title}": ${f.name} (${f.type}) put to ${JSON.stringify(f.toName)}, taken off, and put back to ${JSON.stringify(f.backName)} on GitHub`);
  }
}

// A board grouped by a field: a card dragged into another lane's column takes
// that column's value and that lane's, and is dragged back
async function acrossLanes(run, views) {
  const laned = [];
  for (const [n, layout] of views) if (layout === 'board' && await run(`(() => { const w = I.gh.data.views.find(x => x.number === ${n}); return !!(w.group && w.group !== I.gh.data.columns); })()`)) laned.push(n);
  if (!laned.length) { console.log('5. (no board view grouped into lanes)'); return; }
  const n = laned[0];
  console.log(`5. a card dragged into another lane on view ${n}, and dragged back`);
  await run(`ghpView(${n}); true`);
  await until(() => run(`I.gh.data.view === ${n}`), 'view ' + n, 60000);
  const drag = (title, lane, column) => run(`(() => {
    const lanes = [...document.querySelectorAll("#issuespanel .ghlane")];
    const card = lanes.flatMap(l => [...l.querySelectorAll(".ghcard")]).find(c => c.querySelector(".t").textContent === ${JSON.stringify(title)});
    const into = lanes.find(l => l.querySelector(".ghlanehead .grow").textContent === ${JSON.stringify(lane)});
    const col = into && [...into.querySelectorAll(".ghcol")].find(c => c.querySelector(".ghhead .grow").textContent === ${JSON.stringify(column)});
    if (!card || !col) return false;
    const dt = new DataTransfer();
    card.dispatchEvent(new DragEvent("dragstart", {dataTransfer: dt, bubbles: true}));
    col.dispatchEvent(new DragEvent("dragover", {dataTransfer: dt, bubbles: true, cancelable: true}));
    col.dispatchEvent(new DragEvent("drop", {dataTransfer: dt, bubbles: true, cancelable: true}));
    return true; })()`);
  const plan = JSON.parse(await run(`JSON.stringify((() => { const d = I.gh.data, w = d.views.find(x => x.number === ${n});
    const g = d.fields.find(f => f.id === w.group), col = d.fields.find(f => f.id === d.columns);
    const c = d.cards.find(x => x.kind !== "hidden" && ghpKey(x, g) !== null && ghpKey(x, col) !== null);
    const gTo = (g.options || g.iterations).find(o => o.id !== ghpKey(c, g)), cTo = (col.options || col.iterations).find(o => o.id !== ghpKey(c, col));
    return {card: c.id, title: c.title, lane: g.name, column: col.name, from: [ghpWords(c, g), ghpWords(c, col)], to: [gTo.name || gTo.title, cTo.name || cTo.title]}; })())`));
  check(await drag(plan.title, plan.to[0], plan.to[1]), `"${plan.title}" dragged from ${plan.from.join(' / ')} into ${plan.to.join(' / ')}`);
  await settled(run);
  check(await githubHas(plan.card, plan.lane, plan.to[0]) && await githubHas(plan.card, plan.column, plan.to[1]),
    `GitHub has it in ${plan.lane} ${plan.to[0]} and ${plan.column} ${plan.to[1]}`);
  await sleep(2500);
  check(await drag(plan.title, plan.from[0], plan.from[1]), 'and dragged back');
  await settled(run);
  check(await githubHas(plan.card, plan.lane, plan.from[0]) && await githubHas(plan.card, plan.column, plan.from[1]), 'GitHub has it back where it was');
}

let ws;
try {
  spawn(path.join(APP, 'SHIKISHA-TERM.exe'), ['--behind'], { cwd: APP, env, detached: true, stdio: 'ignore' }).unref();
  await until(() => portOf(), 'the board\'s DevTools port');
  let target;
  await until(async () => {
    const list = await (await fetch(`http://127.0.0.1:${portOf()}/json/list`)).json();
    target = list.find((t) => t.type === 'page' && /127\.0\.0\.1/.test(t.url));
    return !!target;
  }, 'the board page');
  const cdp = await connectCdp(target.webSocketDebuggerUrl);
  ws = cdp.ws;
  const { run, send } = cdp;
  const shot = async (name) => {
    const { data } = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, name + '.png'), Buffer.from(data, 'base64'));
  };
  await until(() => run('typeof S !== "undefined" && !!(S && S.tabs && S.tabs.length)'), 'the board state');

  console.log('1. the Issue tab, switched to Project');
  await run('send({kind:"openissues"}); true');
  await until(() => run('!document.getElementById("issuespanel").hidden && Array.isArray(I.projects) && I.projects.length > 0'), 'the Issue tab', 30000);
  await run('[...document.querySelectorAll("#issuespanel .seg button")].find(b => b.textContent === (T["issues.kind.ghp"] || "")).click(); true');
  check(await run('I.kind === "ghp"'), 'the Project button switches the tab to GitHub Projects');
  // The list of them (nothing chosen yet) or the reason none can be seen
  await until(() => run('I.gh.list !== null || !!I.gh.listErr'), 'the GitHub Projects or why not', 60000);
  const listed = JSON.parse(await run('JSON.stringify({list: I.gh.list, err: I.gh.listErr, fix: I.gh.listFix, hidden: I.gh.hidden})'));
  await shot('1-chosen-from');
  if (listed.err) {
    console.log('   GitHub said: ' + listed.err);
    const denied = await run('T["err.ghproject.denied"]');
    check(listed.err === denied || /organization forbids/.test(listed.err), 'a token that may not see GitHub Projects is told so');
    check(listed.fix && await run('[...document.querySelectorAll("#issuespanel .bwarn button")].length > 0'), 'with the way to the account\'s settings beside it');
  } else {
    console.log(`   ${listed.list.length} listed, ${listed.hidden} hidden: ` + listed.list.map((p) => p.title + ' (' + p.from + ')').join(', '));
    check(listed.list.length > 0, 'the GitHub Projects of the repository are listed');
    const pick = process.env.GHP_PICK || (listed.list[0] || {}).url;
    console.log('2. ' + pick + ' opened');
    await run(`ghpPick(${JSON.stringify(pick)}); true`);
    await until(() => run('!!I.gh.data || !!I.gh.err'), 'the GitHub Project', 90000);
    const read = JSON.parse(await run('JSON.stringify({err: I.gh.err, d: I.gh.data && {title: I.gh.data.title, view: I.gh.data.view, views: I.gh.data.views.map(w => [w.number, w.layout]), cards: I.gh.data.cards.length, columns: I.gh.data.columns}})'));
    await shot('2-opened');
    if (read.err) { check(false, 'the GitHub Project was read: ' + read.err); throw new Error('stop'); }
    console.log('   ' + JSON.stringify(read.d));
    const layout = (read.d.views.find(([n]) => n === read.d.view) || [])[1];
    const drawn = await run('JSON.stringify({cards: document.querySelectorAll("#issuespanel .ghcard").length, rows: document.querySelectorAll("#issuespanel .ght tbody tr:not(.grp)").length, cols: document.querySelectorAll("#issuespanel .ghcol").length})');
    console.log('   drawn: ' + drawn);
    const n = JSON.parse(drawn);
    check(layout === 'board' && read.d.columns ? n.cards === read.d.cards && n.cols > 0 : n.rows === read.d.cards,
      'every card GitHub has is drawn, as the view lays them out');
    // The choice stays with the project
    await until(() => run(`(I.projects.find(p => p.name === "repo") || {}).gh_project === ${JSON.stringify(pick)}`), 'the choice kept in the settings', 15000)
      .then(() => check(true, 'the GitHub Project chosen is kept on the project'))
      .catch(() => check(false, 'the GitHub Project chosen is kept on the project'));
    if (VIEWS) await everyView(run, shot, read.d.views);
    if (WRITE) {
      console.log('3. a card moved, read back, and moved back');
      const plan = JSON.parse(await run(`(() => { const d = I.gh.data; const f = d.fields.find(x => x.id === d.columns);
        const c = d.cards.find(x => x.kind !== "hidden"); if (!f || !c) return "null";
        const now = ghpKey(c, f); const to = (f.options || f.iterations).find(o => o.id !== now);
        return JSON.stringify({card: c.id, field: f.id, now, to: to && to.id, iter: f.type === "ITERATION"}); })()`));
      if (!plan || !plan.to) check(false, 'a card and another column to move it to');
      else {
        const value = (id) => id === null ? null : plan.iter ? { iteration: id } : { option: id };
        const move = async (id) => {
          await run(`(() => { const d = I.gh.data; ghpSet(d.cards.find(x => x.id === ${JSON.stringify(plan.card)}), d.fields.find(x => x.id === ${JSON.stringify(plan.field)}), ${JSON.stringify(value(id))}); return true; })()`);
          await until(() => run(`(() => { const c = (I.gh.data.cards || []).find(x => x.id === ${JSON.stringify(plan.card)}); const f = I.gh.data.fields.find(x => x.id === ${JSON.stringify(plan.field)}); return !!c && ghpKey(c, f) === ${JSON.stringify(id)} && !I.bad; })()`), 'GitHub to have it', 60000);
        };
        await move(plan.to);
        await sleep(3000);
        check(await run(`(() => { const c = I.gh.data.cards.find(x => x.id === ${JSON.stringify(plan.card)}); return ghpKey(c, I.gh.data.fields.find(x => x.id === ${JSON.stringify(plan.field)})) === ${JSON.stringify(plan.to)}; })()`),
          'the card is in the other column as GitHub reads it back');
        await shot('3-moved');
        await move(plan.now);
        check(true, 'the card was moved back');
      }
      if (VIEWS) { await everyField(run); await acrossLanes(run, read.d.views); }
    }
  }
} catch (e) {
  if (e.message !== 'stop') { console.error(e); failures += 1; }
} finally {
  try { ws && ws.close(); } catch (e) {}
  if (!process.env.GHP_KEEP) stopApp();
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
