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
// With GHP_WRITE=1: cards added, written, put in order, archived and brought
// back, and taken off, each through the tab; GHP_ISSUE (owner/repo#n) is an
// issue put on the GitHub Project for a moment and taken off again
const CARDS = process.env.GHP_CARDS === '1';
const ISSUE = process.env.GHP_ISSUE || '';
// With GHP_WRITE=1: the board itself changed through the tab and read back
// from GitHub each time -- a field added, its choices changed and a card
// given several of them, a column added to a board, a view added, renamed,
// given a filter and deleted, a draft assigned, the archive opened and a
// card brought back from it, the board's own words and a report of how it
// stands, and a board made, copied and deleted. Everything it makes is
// named "Check ..." and taken away again. GHP_ISSUE, an issue of a
// repository that is not GHP_REPO, is opened on the tab from its card
const BOARD = process.env.GHP_BOARD === '1';

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
    // A card by its name in GitHub's answer; an empty one for a name it does not have
    const named = (title) => real.find((c) => c.title === title) || { values: {}, people: '' };
    if (layout === 'roadmap' && await run('!!document.querySelector("#issuespanel .ghroad")')) {
      // On the line of time: every card by name, a bar from its start to its
      // end where it has both, a point where it has one, none where it has neither
      const drawn = JSON.parse(await run(`JSON.stringify({by: [...document.querySelectorAll("#issuespanel .bar.sub .faint")].map(e => e.textContent).join(" | "),
        rows: [...document.querySelectorAll("#issuespanel .ghroad .ghrrow:not(.ghrhead):not(.ghrgroup)")].map(r => { const b = r.querySelector(".ghrbar");
          return {title: r.querySelector(".ttl").textContent, bar: b ? {left: b.offsetLeft, width: b.offsetWidth, point: b.classList.contains("point")} : null}; })})`));
      console.log('     ' + drawn.by);
      check(JSON.stringify(drawn.rows.map((r) => r.title).sort()) === JSON.stringify(titles), `view ${n}: the timeline has every card GitHub's filter leaves (${drawn.rows.length})`);
      const ends = await run('JSON.stringify((() => { const d = I.gh.data, w = d.views.find(x => x.number === d.view), b = ghpRoadFields(d, w); return b.iter ? [b.iter.name, null, "iter"] : b.point ? [b.point.name, null, "point"] : [b.start.name, b.end.name, "range"]; })())');
      const [from, to, kind] = JSON.parse(ends);
      // A card GitHub's answer does not have by that name (one the token
      // cannot see, or two with one name) cannot be checked against it
      const of = (title) => named(title).values;
      let ok = true;
      for (const r of drawn.rows) {
        const v = of(r.title);
        const want = kind === 'iter' ? (v[from] ? 'bar' : 'none') : kind === 'point' ? (v[from] ? 'point' : 'none')
          : v[from] && v[to] ? 'bar' : v[from] || v[to] ? 'point' : 'none';
        const got = !r.bar ? 'none' : r.bar.point ? 'point' : 'bar';
        if (want !== got) { ok = false; console.log(`     "${r.title}" is drawn as ${got}, GitHub's ${from}/${to} make it a ${want}`); }
      }
      check(ok, kind === 'range' ? `view ${n}: a bar for each card with ${from} and ${to}, a point for one with only one, nothing for one with neither`
        : `view ${n}: each card with ${from} drawn by it, nothing for one without`);
      // Iterations are named, not dated, in GitHub's answer: their order is the field's
      if (kind !== 'iter') {
        const placed = drawn.rows.filter((r) => r.bar).map((r) => ({ at: of(r.title), ...r.bar }))
          .map((r) => ({ day: [r.at[from], to && r.at[to]].filter(Boolean).sort()[0] || '', left: r.left })).sort((a, b) => a.day.localeCompare(b.day) || a.left - b.left);
        check(placed.every((p, i) => !i || p.left >= placed[i - 1].left), `view ${n}: the bars begin further along the earlier their first day is`);
      }
      continue;
    }
    if (layout === 'board') {
      const drawn = JSON.parse(await run(`JSON.stringify([...document.querySelectorAll("#issuespanel .ghlane")].length
        ? [...document.querySelectorAll("#issuespanel .ghlane")].map(l => ({lane: l.querySelector(".ghlanehead .grow").textContent, cols: [...l.querySelectorAll(".ghcol")].map(c => ({name: c.querySelector(".ghhead .grow").textContent, cards: [...c.querySelectorAll(".ghcard .t")].map(t => t.textContent)}))}))
        : [{lane: null, cols: [...document.querySelectorAll("#issuespanel .ghcol")].map(c => ({name: c.querySelector(".ghhead .grow").textContent, cards: [...c.querySelectorAll(".ghcard .t")].map(t => t.textContent)}))}])`));
      const all = drawn.flatMap((l) => l.cols.flatMap((c) => c.cards)).sort();
      check(JSON.stringify(all) === JSON.stringify(titles), `view ${n}: the cards are the ones GitHub's filter leaves (${all.length})`);
      let placed = true;
      for (const l of drawn) for (const c of l.cols) for (const t of c.cards) {
        const card = named(t);
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
      const drawn = JSON.parse(await run(`JSON.stringify([...document.querySelectorAll("#issuespanel .ght tbody tr:not(.addrow)")].map(r => r.classList.contains("grp") ? {group: r.firstChild.firstChild.textContent.trim()} : {title: r.querySelector(".ttl").textContent, people: ${who} < 0 ? null : r.children[${who}].textContent}))`));
      if (who >= 0) {
        const wrong = drawn.filter((r) => r.title !== undefined && r.people !== named(r.title).people);
        for (const r of wrong) console.log(`     "${r.title}" shows "${r.people}" as its assignees, GitHub has "${named(r.title).people}"`);
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
          const want = named(r.title).values[v.group] ?? T_NONE(v.group);
          if (want !== lane) { ok = false; console.log(`     "${r.title}" is under ${lane}, GitHub has ${want}`); }
        }
        check(ok, `view ${n}: the rows are grouped by ${v.group}: ${groups.join(' | ')}`);
      }
      for (const [name, type, desc] of v.sort) {
        let ok = true, lane = null, last;
        for (const r of drawn) {
          if (r.group !== undefined) { lane = r.group; last = undefined; continue; }
          const x = named(r.title).values[name];
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
// Every change sent has been answered, and none was refused: a refusal is
// said here in GitHub's words, since nothing after it would mean anything
const settled = async (run) => {
  await until(() => run('!I.gh.sending.length'), 'the changes to reach GitHub', 60000);
  if (await run('I.bad')) throw new Error('refused: ' + await run('I.said'));
};
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

// The GitHub Project's cards as GitHub has them now, in its own order: each
// card's id, what it is, whether it is archived, a draft's words, its values
const board = async (projectId) => {
  const field = '... on ProjectV2FieldCommon { name }';
  const r = await (await fetch('https://api.github.com/graphql', { method: 'POST', headers: { Authorization: 'bearer ' + PAT },
    body: JSON.stringify({ query: `query($id: ID!) { node(id: $id) { ... on ProjectV2 { items(first: 100, orderBy: {field: POSITION, direction: ASC}) { nodes {
      id isArchived content { __typename ... on DraftIssue { title body } ... on Issue { title number repository { nameWithOwner } } ... on PullRequest { title number repository { nameWithOwner } } }
      fieldValues(first: 50) { nodes { ... on ProjectV2ItemFieldSingleSelectValue { name field { ${field} } } } } } } } } }`, variables: { id: projectId } }) })).json();
  if (r.errors) throw new Error(JSON.stringify(r.errors));
  return r.data.node.items.nodes.map((i) => ({ id: i.id, archived: i.isArchived, kind: (i.content || {}).__typename, title: (i.content || {}).title,
    body: (i.content || {}).body, number: (i.content || {}).number, repo: ((i.content || {}).repository || {}).nameWithOwner,
    values: Object.fromEntries(i.fieldValues.nodes.filter((v) => v.field).map((v) => [v.field.name, v.name])) }));
};
// Archived cards are not among a board's items; one is asked for by its id
const isArchived = async (id) => {
  const r = await (await fetch('https://api.github.com/graphql', { method: 'POST', headers: { Authorization: 'bearer ' + PAT },
    body: JSON.stringify({ query: 'query($id: ID!) { node(id: $id) { ... on ProjectV2Item { isArchived } } }', variables: { id } }) })).json();
  return r.data && r.data.node ? r.data.node.isArchived : null;
};
const eventually = async (test, ms = 30000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test()) return true; await sleep(1000); }
  return false;
};

// Every way a card is changed besides its values, as a person does it on
// the tab: typed into a column's box, written on the draft's page, moved by
// its ⋯, archived and brought back, an issue put on and taken off
async function everyCardAct(run, shot, views) {
  console.log('6. cards added, written, put in order, archived, brought back and taken off, through the tab');
  const projectId = await run('I.gh.data.id');
  const stamp = 'Check ' + new Date().toISOString().slice(11, 19);
  const laned = [];
  for (const [n, layout] of views) if (layout === 'board' && await run(`(() => { const w = I.gh.data.views.find(x => x.number === ${n}); return !!(w.group && w.group !== I.gh.data.columns); })()`)) laned.push(n);
  const table = (views.find(([, l]) => l === 'table') || [])[0];
  // Added in a lane's column on a board, by typing into its box
  if (laned.length) {
    const n = laned[0];
    await run(`ghpView(${n}); true`);
    await until(() => run(`I.gh.data.view === ${n} && !I.gh.sending.length`), 'view ' + n, 60000);
    const where = JSON.parse(await run(`JSON.stringify((() => { const l = document.querySelectorAll("#issuespanel .ghlane")[0];
      const col = [...l.querySelectorAll(".ghcol")][1]; return {lane: l.querySelector(".ghlanehead .grow").textContent, column: col.querySelector(".ghhead .grow").textContent}; })())`));
    await run(`(() => { const col = [...document.querySelectorAll("#issuespanel .ghlane")[0].querySelectorAll(".ghcol")][1]; col.querySelector(".ghadd button").click(); return true; })()`);
    await sleep(300);
    const typed = await run(`(() => { const b = document.querySelector('#issuespanel [data-k="ghpadd"]'); if (!b) return false;
      b.value = ${JSON.stringify(stamp)}; b.dispatchEvent(new Event("input"));
      b.dispatchEvent(new KeyboardEvent("keydown", {key: "Enter", bubbles: true})); return true; })()`);
    check(typed, `a draft typed into the box under ${where.lane} / ${where.column}`);
    await settled(run);
    const lanes = JSON.parse(await run(`JSON.stringify((() => { const w = I.gh.data.views.find(x => x.number === ${n}); return [I.gh.data.fields.find(f => f.id === w.group).name, I.gh.data.fields.find(f => f.id === I.gh.data.columns).name]; })())`));
    check(await eventually(async () => { const c = (await board(projectId)).find((x) => x.title === stamp);
      return c && c.kind === 'DraftIssue' && c.values[lanes[0]] === where.lane && c.values[lanes[1]] === where.column; }),
      `GitHub has the draft in ${lanes[0]} ${where.lane} and ${lanes[1]} ${where.column}`);
    await until(() => run(`!!I.gh.data.cards.find(c => c.title === ${JSON.stringify(stamp)} && c.kind === "draft")`), 'the draft read back', 30000);
    await shot('6-added');
  } else {
    await run(`ghpView(${table}); true`);
    await until(() => run(`I.gh.data.view === ${table}`), 'view ' + table, 60000);
    await run(`ghpAdd(${JSON.stringify(stamp)}, []); true`);
    await settled(run);
    await until(() => run(`!!I.gh.data.cards.find(c => c.title === ${JSON.stringify(stamp)} && c.kind === "draft")`), 'the draft read back', 30000);
  }
  const id = await run(`I.gh.data.cards.find(c => c.title === ${JSON.stringify(stamp)}).id`);
  // Written on its page: opened by pressing it, its title and words changed and saved
  // (a press moves the focus out of the box it was typed in, as a hand does)
  await run('document.activeElement && document.activeElement.blur(); true');
  await run(`(() => { const c = [...document.querySelectorAll("#issuespanel .ghcard, #issuespanel .ttl")].find(e => e.textContent.includes(${JSON.stringify(stamp)})); c.click(); return true; })()`);
  await until(() => run('!!document.querySelector(\'#issuespanel [data-k="ghpdt"]\')'), 'the draft page', 10000);
  await shot('6-draft');
  const title = stamp + ' (written)', words = 'Written on the Issue tab.\n\nSecond paragraph.';
  await run(`(() => { const t = document.querySelector('#issuespanel [data-k="ghpdt"]'), b = document.querySelector('#issuespanel [data-k="ghpdb"]');
    t.value = ${JSON.stringify(title)}; t.dispatchEvent(new Event("input")); b.value = ${JSON.stringify(words)}; b.dispatchEvent(new Event("input"));
    [...document.querySelectorAll("#issuespanel .ghdraft .foot button")].find(x => x.classList.contains("go")).click(); return true; })()`);
  await settled(run);
  check(await eventually(async () => { const c = (await board(projectId)).find((x) => x.id === id); return c && c.title === title && c.body === words; }),
    'the draft\'s title and words, written on its page, are on GitHub');
  // Made an issue of a repository that is not there: nothing is made, the
  // words go in first all the same, and the page comes back with them
  await run(`ghpEdit(I.gh.data.cards.find(c => c.id === ${JSON.stringify(id)})); true`);
  await until(() => run('!!document.querySelector(\'#issuespanel [data-k="ghpdb"]\')'), 'the draft page', 10000);
  const later = words + '\n\nThird paragraph, sent with the issue.';
  await run(`(() => { const b = document.querySelector('#issuespanel [data-k="ghpdb"]'); b.value = ${JSON.stringify(later)}; b.dispatchEvent(new Event("input"));
    I.gh.edit.repo = "te0ai/no-such-repository-for-the-check"; ghpToIssue(I.gh.data.cards.find(c => c.id === ${JSON.stringify(id)})); return true; })()`);
  await until(() => run('!I.gh.sending.length'), 'the answer', 60000);
  check(await run('I.bad && !!I.said'), 'an issue that cannot be made says why');
  check(await run(`!!I.gh.edit && I.gh.edit.body === ${JSON.stringify(later)} && !!document.querySelector('#issuespanel [data-k="ghpdb"]')`),
    'and the draft\'s page is back, with the words as they were written');
  check(await eventually(async () => { const c = (await board(projectId)).find((x) => x.id === id); return c && c.kind === 'DraftIssue' && c.body === later; }),
    'the words went in first all the same; the card is still a draft');
  await run('I.gh.edit = null; I.said = ""; I.bad = false; drawIssues(); true');
  // Put in order by its ⋯, in a view in the board's own order
  await run(`ghpView(${table}); true`);
  await until(() => run(`I.gh.data.view === ${table} && !I.gh.sending.length`), 'view ' + table, 60000);
  // A card's ⋯, by the card's own id: never another card with the same title
  const menuOf = (card, label) => run(`(() => { const r = document.querySelector('#issuespanel .ght tbody tr[data-card="' + ${JSON.stringify(card)} + '"]');
    if (!r) return false;
    r.querySelector("td.act button").click(); const e = [...document.querySelectorAll(".fmenu > div")].find(x => x.textContent === T[${JSON.stringify(label)}]);
    if (!e) { closeFolderMenu(); return false; } e.click(); return true; })()`);
  const menu = (label) => menuOf(id, label);
  const sorted = await run(`!ghpByHand(I.gh.data.views.find(x => x.number === ${table}))`);
  if (sorted) console.log(`     (view ${table} sorts its cards: no order by hand there)`);
  else {
    const before = (await board(projectId)).map((c) => c.id).indexOf(id);
    check(await menu('ghp.up'), 'Move up is on its ⋯');
    await settled(run);
    check(await eventually(async () => (await board(projectId)).map((c) => c.id).indexOf(id) === before - 1), `GitHub has it one place up (${before} → ${before - 1})`);
    await sleep(2500);
    check(await menu('ghp.down'), 'Move down is on its ⋯');
    await settled(run);
    check(await eventually(async () => (await board(projectId)).map((c) => c.id).indexOf(id) === before), 'and back down where it was');
    await sleep(2500);
  }
  // Archived by its ⋯, and brought back with the press beside the words that say so
  check(await menu('ghp.archive'), 'Archive is on its ⋯');
  check(await run(`!I.gh.data.cards.some(c => c.id === ${JSON.stringify(id)}) && !!document.querySelector("#issuespanel .ghundo button")`), 'it leaves the view at once, with Undo beside it');
  await settled(run);
  check(await eventually(async () => (await isArchived(id)) === true), 'GitHub has it archived');
  await shot('6-archived');
  await run('document.querySelector("#issuespanel .ghundo button").click(); true');
  await settled(run);
  check(await eventually(async () => (await isArchived(id)) === false), 'Undo brings it back on GitHub');
  await until(() => run(`I.gh.data.cards.some(c => c.id === ${JSON.stringify(id)})`), 'it back in the view', 30000);
  // An issue already on GitHub, put on by its name, then taken off -- the
  // issue itself left as it was
  if (ISSUE) {
    // The card the issue becomes is the one that was not there before: only
    // that one is taken off again, whatever else has the same number or title
    const [repo, n] = ISSUE.split('#');
    const num = Number(n);
    const already = new Set((await board(projectId)).map((c) => c.id));
    if ((await board(projectId)).some((c) => c.repo === repo && c.number === num)) { check(false, `${ISSUE} is already on the project: not used`); }
    else {
      await run(`ghpAdd(${JSON.stringify(ISSUE)}, []); true`);
      await settled(run);
      let added = null;
      check(await eventually(async () => { added = (await board(projectId)).find((c) => !already.has(c.id) && c.repo === repo && c.number === num); return !!added; }),
        `${ISSUE}, typed by its name, is a card on GitHub`);
      if (added) {
        // Read back from GitHub, no longer the card shown waiting (which has no ⋯ yet)
        await until(() => run(`I.gh.data.cards.some(c => c.id === ${JSON.stringify(added.id)} && c.kind !== "adding")`), 'the issue read back', 30000);
        check(await menuOf(added.id, 'ghp.remove'), 'Take off the project is on its ⋯');
        check(await run('!!document.querySelector("#issuespanel .bwarn button.bad")'), 'taking it off asks first');
        await shot('6-remove-ask');
        await run('document.querySelector("#issuespanel .bwarn button.bad").click(); true');
        await settled(run);
        check(await eventually(async () => { const now = await board(projectId); return !now.some((c) => c.id === added.id) && [...already].every((x) => now.some((c) => c.id === x)); }),
          'and once yes is pressed, GitHub no longer has it on the project, and every other card is still there');
      }
    }
  }
  // The draft taken off: nothing of the check left behind
  check(await menu('ghp.remove'), 'Take off the project is on its ⋯');
  await run('document.querySelector("#issuespanel .bwarn button.bad").click(); true');
  await settled(run);
  check(await eventually(async () => !(await board(projectId)).some((c) => c.id === id) && (await isArchived(id)) === null), 'the draft is gone from GitHub');
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

const gq = async (query, variables) => {
  const r = await (await fetch('https://api.github.com/graphql', { method: 'POST', headers: { Authorization: 'bearer ' + PAT },
    body: JSON.stringify({ query, variables }) })).json();
  if (r.errors && !r.data) throw new Error(JSON.stringify(r.errors));
  return r.data;
};
// The board as GitHub has it: its fields with their choices, its views, its words
const boardShape = async (id) => (await gq(`query($id: ID!) { node(id: $id) { ... on ProjectV2 { title shortDescription
  fields(first: 50) { nodes { ... on ProjectV2FieldCommon { id name dataType } ... on ProjectV2SingleSelectField { options { id name color } }
    ... on ProjectV2MultiSelectField { multiSelectOptions { id name color } } } }
  views(first: 30) { nodes { id number name layout filter } }
  statusUpdates(first: 10) { nodes { id body status } } } } }`, { id })).node;

// The board itself, changed the way a person changes it on the tab
async function everyBoardAct(run, shot, views) {
  console.log('7. the board itself: fields, a column, views, a draft\'s people, the archive, its words, a board made and deleted');
  const projectId = await run('I.gh.data.id');
  const stamp = 'Check ' + new Date().toISOString().slice(11, 19).replaceAll(':', '');
  const press = (sel) => run(`(() => { const b = document.querySelector(${JSON.stringify(sel)}); if (!b) return false; b.click(); return true; })()`);
  const type = (sel, text, key) => run(`(() => { const b = document.querySelector(${JSON.stringify(sel)}); if (!b) return false;
    b.focus(); b.value = ${JSON.stringify(text)}; b.dispatchEvent(new Event("input")); b.dispatchEvent(new Event("change"));
    ${key ? `b.dispatchEvent(new KeyboardEvent("keydown", {key: ${JSON.stringify(key)}, bubbles: true}));` : ''} return true; })()`);
  const pressText = async (text) => {
    const done = await run(`(() => { const b = [...document.querySelectorAll("#issuespanel button")].find(x => x.textContent.trim() === ${JSON.stringify(text)}); if (!b || b.disabled) return false; b.click(); return true; })()`);
    // What was there instead, when it was not
    if (!done) console.log('     no "' + text + '" to press among: ' + await run(`[...document.querySelectorAll("#issuespanel button")].map(x => x.textContent.trim() + (x.disabled ? "(off)" : "")).join(" | ").slice(0, 600)`));
    return done;
  };
  const t = (key) => run(`T[${JSON.stringify(key)}] || ""`);
  const page = async (name) => { await run(`document.activeElement && document.activeElement.blur(); Object.assign(I.gh, {panel: ${JSON.stringify(name)}}); ${name === 'about' ? 'ghpAboutAsk();' : ''} issuesSig = ""; drawIssues(); true`); await sleep(300); };
  const before = await boardShape(projectId);
  const statusBefore = before.fields.nodes.find((f) => f.name === 'Status');

  // A field of several choices: added from the fields page, a card given
  // two of them, a choice renamed, the field deleted
  await page('fields');
  const fname = stamp + ' area';
  await type('#issuespanel [data-k="ghpfnew"]', fname);
  await run(`(() => { const s = document.querySelector('#issuespanel .ghset select'); const all = [...document.querySelectorAll('#issuespanel .sect')].pop().querySelector('select');
    document.activeElement.blur(); all.value = "MULTI_SELECT"; all.dispatchEvent(new Event("change")); return true; })()`);
  await sleep(300);
  await type('#issuespanel [data-k="ghpfnewo"]', 'Web, App, Email');
  check(await pressText(await t('ghp.fields.add')), 'a multi-select field typed into the fields page and added');
  await settled(run);
  let made;
  check(await eventually(async () => { made = (await boardShape(projectId)).fields.nodes.find((f) => f.name === fname);
    return made && made.dataType === 'MULTI_SELECT' && made.multiSelectOptions.map((o) => o.name).join() === 'Web,App,Email'; }),
    'GitHub has the field, with its three choices in order');
  await until(() => run(`!!I.gh.data.fields.find(f => f.name === ${JSON.stringify(fname)})`), 'the field read back', 30000);
  await run(`Object.assign(I.gh, {panel: ""}); issuesSig = ""; drawIssues(); true`);
  const draft = JSON.parse(await run(`JSON.stringify((() => { const c = I.gh.data.cards.find(x => x.kind === "draft"); return c ? {id: c.id, title: c.title} : null; })())`));
  if (!draft) check(false, 'a draft card to give values to');
  else {
    await run(`(() => { const d = I.gh.data, f = d.fields.find(x => x.name === ${JSON.stringify(fname)}), c = d.cards.find(x => x.id === ${JSON.stringify(draft.id)});
      ghpSet(c, f, {options: [f.options[0].id, f.options[2].id]}); return true; })()`);
    await settled(run);
    const picked = async () => ((await gq(`query($id: ID!) { node(id: $id) { ... on ProjectV2Item { fieldValues(first: 50) { nodes {
      ... on ProjectV2ItemFieldMultiSelectValue { options { name } field { ... on ProjectV2FieldCommon { name } } } } } } } }`, { id: draft.id }))
      .node.fieldValues.nodes.find((v) => v.field && v.field.name === fname) || { options: [] }).options.map((o) => o.name).join();
    check(await eventually(async () => await picked() === 'Web,Email'), 'a card given two of its choices has them on GitHub');
    check(await run(`(() => { const d = I.gh.data, f = d.fields.find(x => x.name === ${JSON.stringify(fname)}), c = d.cards.find(x => x.id === ${JSON.stringify(draft.id)});
      return ghpWords(c, f) === "Web, Email"; })()`), 'and the tab shows them as GitHub reads them back');
  }
  await page('fields');
  await run(`(() => { const f = I.gh.data.fields.find(x => x.name === ${JSON.stringify(fname)}); ghpFieldOpen(f); I.gh.fedit.options[1].name = "Mobile"; I.gh.fedit.options[1].color = "PURPLE"; issuesSig = ""; drawIssues(); return true; })()`);
  await sleep(200);
  check(await pressText(await t('ghp.save')), 'a choice renamed and recoloured in the field\'s editor');
  await settled(run);
  check(await eventually(async () => { const f = (await boardShape(projectId)).fields.nodes.find((x) => x.name === fname);
    return f && f.multiSelectOptions.map((o) => o.name + ':' + o.color).join() === 'Web:GRAY,Mobile:PURPLE,Email:GRAY'
      && f.multiSelectOptions.map((o) => o.id).join() === made.multiSelectOptions.map((o) => o.id).join(); }),
    'GitHub has the choice renamed, every choice still by the id it had');
  await until(() => run(`!I.gh.fedit && !I.gh.sending.length`), 'the editor closed', 30000);
  await run(`I.gh.fdel = I.gh.data.fields.find(x => x.name === ${JSON.stringify(fname)}).id; issuesSig = ""; drawIssues(); true`);
  check(await press('#issuespanel .ghset .bwarn button.bad'), 'the field deleted after a yes');
  await settled(run);
  check(await eventually(async () => !(await boardShape(projectId)).fields.nodes.some((f) => f.name === fname)), 'GitHub no longer has the field');

  // A column added at the end of a board, and taken off again in the field's editor
  const boardView = (views.find(([, l]) => l === 'board') || [])[0];
  await run(`Object.assign(I.gh, {panel: ""}); ghpView(${boardView}); true`);
  await until(() => run(`I.gh.data.view === ${boardView} && !I.gh.sending.length && I.gh.data.columns === ${JSON.stringify(statusBefore.id)}`), 'the board by Status', 60000).catch(() => {});
  if (await run(`I.gh.data.columns === ${JSON.stringify(statusBefore.id)} && !!document.querySelector("#issuespanel .ghcol.new button")`)) {
    await press('#issuespanel .ghcol.new button');
    await sleep(300);
    check(await type('#issuespanel [data-k="ghpcol"]', stamp + ' col', 'Enter'), 'a column typed at the end of the board');
    await settled(run);
    check(await eventually(async () => { const f = (await boardShape(projectId)).fields.nodes.find((x) => x.id === statusBefore.id);
      return f.options.length === statusBefore.options.length + 1 && f.options[f.options.length - 1].name === stamp + ' col'
        && statusBefore.options.every((o, i) => f.options[i].id === o.id && f.options[i].name === o.name && f.options[i].color === o.color); }),
      'GitHub has the new choice last, the others as they were, by the ids they had');
    await until(() => run(`[...document.querySelectorAll("#issuespanel .ghhead .grow")].some(h => h.textContent === ${JSON.stringify(stamp + ' col')})`), 'the column drawn', 30000)
      .then(() => check(true, 'the board shows the new column')).catch(() => check(false, 'the board shows the new column'));
    await page('fields');
    await run(`(() => { const f = I.gh.data.fields.find(x => x.id === ${JSON.stringify(statusBefore.id)}); ghpFieldOpen(f); I.gh.fedit.options.pop(); issuesSig = ""; drawIssues(); return true; })()`);
    await pressText(await t('ghp.save'));
    await settled(run);
    check(await eventually(async () => { const f = (await boardShape(projectId)).fields.nodes.find((x) => x.id === statusBefore.id);
      return f.options.map((o) => o.id + o.name).join() === statusBefore.options.map((o) => o.id + o.name).join(); }), 'and Status is as it was once the column is taken off');
  } else check(false, 'a board in columns by Status to add a column to');

  // A view: added, renamed, given a filter kept on GitHub, deleted
  await page('views');
  await type('#issuespanel [data-k="ghpvnew"]', stamp + ' view');
  check(await pressText(await t('ghp.views.add')), 'a view typed into the views page and added');
  await settled(run);
  let view;
  check(await eventually(async () => { view = (await boardShape(projectId)).views.nodes.find((w) => w.name === stamp + ' view'); return !!view && view.layout === 'TABLE_LAYOUT'; }),
    'GitHub has the view, a table');
  await until(() => run(`!!view0() `.replace('view0()', `I.gh.data.views.find(w => w.name === ${JSON.stringify(stamp + ' view')})`)), 'the view read back', 30000);
  check(await run(`I.gh.data.view === ${view ? view.number : -1} && I.gh.panel === ""`), 'and the tab has gone to the new view');
  await type('#issuespanel [data-k="ghpfilter"]', 'is:draft', 'Enter');
  await until(() => run('I.gh.filter === "is:draft" && !!I.gh.data && !I.busy'), 'the filter applied', 60000);
  check(await pressText(await t('ghp.filter.keep')), 'a filter written here kept as the view\'s own');
  await settled(run);
  check(await eventually(async () => ((await boardShape(projectId)).views.nodes.find((w) => w.id === view.id) || {}).filter === 'is:draft'), 'GitHub has the filter on the view');
  await page('views');
  await run(`(() => { const w = I.gh.data.views.find(x => x.id === ${JSON.stringify(view.id)}); const b = document.querySelector('#issuespanel [data-k="ghpvn' + w.number + '"]');
    b.focus(); b.value = ${JSON.stringify(stamp + ' renamed')}; b.dispatchEvent(new Event("change")); b.blur(); return true; })()`);
  await run(`(() => { const w = I.gh.data.views.find(x => x.id === ${JSON.stringify(view.id)}); w.layout = "board"; ghpSend("gh_project_view_set", {view_id: w.id, change: {layout: "board"}}); return true; })()`);
  await settled(run);
  check(await eventually(async () => { const w = (await boardShape(projectId)).views.nodes.find((x) => x.id === view.id); return w && w.name === stamp + ' renamed' && w.layout === 'BOARD_LAYOUT'; }),
    'GitHub has the view renamed and made a board');
  await page('views');
  await run(`I.gh.vdel = ${view.number}; issuesSig = ""; drawIssues(); true`);
  check(await press('#issuespanel .ghset .bwarn button.bad'), 'the view deleted after a yes');
  await settled(run);
  check(await eventually(async () => (await boardShape(projectId)).views.nodes.map((w) => w.id).join() === before.views.nodes.map((w) => w.id).join()), 'GitHub has the views it had before');
  await until(() => run('!!I.gh.data && I.gh.data.view !== ' + view.number + ' && !I.gh.sending.length'), 'another view shown', 60000);

  // A draft assigned to the account itself from its cell, and to nobody again
  if (draft) {
    const me = (await gq('query { viewer { login } }')).viewer.login;
    const people = async () => ((await gq(`query($id: ID!) { node(id: $id) { ... on ProjectV2Item { content { ... on DraftIssue { assignees(first: 5) { nodes { login } } } } } } }`, { id: draft.id }))
      .node.content.assignees.nodes.map((a) => a.login).join());
    const was = await people();
    await run(`(() => { const c = I.gh.data.cards.find(x => x.id === ${JSON.stringify(draft.id)}); ghpOwnSet(c, "assignees", [${JSON.stringify(me)}]); return true; })()`);
    await settled(run);
    check(await eventually(async () => await people() === me), 'a draft assigned from its cell is assigned on GitHub');
    await run(`(() => { const c = I.gh.data.cards.find(x => x.id === ${JSON.stringify(draft.id)}); ghpOwnSet(c, "assignees", ${JSON.stringify(was ? was.split(',') : [])}); return true; })()`);
    await settled(run);
    check(await eventually(async () => await people() === was), 'and assigned as it was again');
  }

  // The archive: a card put away is listed there, and brought back from it
  await run(`Object.assign(I.gh, {panel: ""}); issuesSig = ""; drawIssues(); true`);
  await run(`ghpAdd(${JSON.stringify(stamp + ' archive me')}, []); true`);
  await settled(run);
  let temp;
  check(await eventually(async () => { temp = (await board(projectId)).find((c) => c.title === stamp + ' archive me'); return !!temp; }), 'a draft added to be put away');
  await until(() => run(`!!I.gh.data.cards.find(c => c.id === ${JSON.stringify(temp.id)} && c.kind === "draft")`), 'the draft read back', 30000);
  await run(`ghpArchive(I.gh.data.cards.find(c => c.id === ${JSON.stringify(temp.id)})); true`);
  await settled(run);
  check(await eventually(async () => await isArchived(temp.id) === true), 'archived on GitHub');

  await run(`ghpArchiveShow(true); true`);
  check(await until(() => run(`!!I.gh.data && I.gh.data.archived === true && I.gh.data.cards.some(c => c.id === ${JSON.stringify(temp.id)})`), 'the archive', 90000).catch(() => false),
    'the archive lists the card put away');
  console.log('   ' + await run('JSON.stringify({archive: I.gh.archive, read: I.gh.data && I.gh.data.archived, cards: I.gh.data && I.gh.data.cards.map(c => c.title), err: I.gh.err, said: I.said, busy: I.busy, sending: I.gh.sending.length, expect: I.gh.expect.length})'));
  await shot('7-archive');
  check(await run(`(() => { const r = document.querySelector('#issuespanel tr[data-card="${temp ? temp.id : ''}"] button'); if (!r) return false; r.click(); return true; })()`), 'Restore pressed on its row');
  await settled(run);
  check(await eventually(async () => await isArchived(temp.id) === false), 'GitHub has it back on the board');
  await run(`ghpArchiveShow(false); true`);
  await until(() => run(`!!I.gh.data && I.gh.data.archived === false && I.gh.data.cards.some(c => c.id === ${JSON.stringify(temp.id)})`), 'the board again', 90000);
  await run(`ghpRemove(I.gh.data.cards.find(c => c.id === ${JSON.stringify(temp.id)})); true`);
  await settled(run);

  // Its own words and a report of how it stands
  await page('about');
  await until(() => run('!!I.gh.about || !!I.gh.aboutErr'), 'what the board says of itself', 60000);
  const about = JSON.parse(await run('JSON.stringify({err: I.gh.aboutErr, a: I.gh.about && {title: I.gh.about.title, can: I.gh.about.can_update, flows: I.gh.about.workflows && I.gh.about.workflows.length, updates: I.gh.about.updates.length, description: I.gh.about.description}})'));
  console.log('   ' + JSON.stringify(about));
  check(!about.err && about.a && about.a.title === before.title && about.a.can, 'the board\'s own page is read, and says this account may change it');
  check(about.a && about.a.flows > 0, 'its workflows are listed');
  await shot('7-about');
  await type('#issuespanel [data-k="ghpadescription"]', stamp + ' words');
  check(await run(`(() => { const b = [...document.querySelectorAll("#issuespanel .sect")][0].querySelector("button.go"); if (!b || b.disabled) return false; b.click(); return true; })()`), 'the short description rewritten and saved');
  await settled(run);
  check(await eventually(async () => (await boardShape(projectId)).shortDescription === stamp + ' words'), 'GitHub has the new description');
  await run(`ghpSend("gh_project_update", {change: {description: ${JSON.stringify(before.shortDescription || '')}}}); true`);
  await settled(run);
  check(await eventually(async () => ((await boardShape(projectId)).shortDescription || '') === (before.shortDescription || '')), 'and the one it had again');
  await page('about');
  await until(() => run('!!I.gh.about'), 'the board\'s page again', 60000);
  await type('#issuespanel [data-k="ghpsb"]', stamp + ' report');
  check(await pressText(await t('ghp.status.write')), 'a report of how it stands written');
  await settled(run);
  let report;
  check(await eventually(async () => { report = (await boardShape(projectId)).statusUpdates.nodes.find((s) => s.body === stamp + ' report'); return !!report && report.status === 'ON_TRACK'; }),
    'GitHub has the report, on track');
  await until(() => run(`!!I.gh.about && I.gh.about.updates.some(u => u.id === ${JSON.stringify(report ? report.id : '')})`), 'the report listed', 30000)
    .then(() => check(true, 'the page lists it')).catch(() => check(false, 'the page lists it'));
  await run(`ghpSend("gh_project_status_remove", {update: ${JSON.stringify(report ? report.id : '')}}); true`);
  await settled(run);
  check(await eventually(async () => !(await boardShape(projectId)).statusUpdates.nodes.some((s) => s.body === stamp + ' report')), 'and deleted again');

  // An issue of a repository the desk has no project for: opened on the tab
  if (ISSUE && ISSUE.split('#')[0].toLowerCase() !== REPO.toLowerCase()) {
    const [repo, n] = ISSUE.split('#');
    await run(`document.activeElement && document.activeElement.blur(); Object.assign(I.gh, {panel: ""}); issueOpenAt(I.gh.project, ${JSON.stringify(repo)}, ${Number(n)}, "issue"); true`);
    check(await until(() => run(`I.view === "detail" && !!I.detail && I.detail.at_repo === ${JSON.stringify(repo)} && I.detail.number === ${Number(n)} && !!I.detail.title`), 'the issue', 60000).catch(() => false),
      `${ISSUE}, of a repository the desk has no project for, opens on this tab`);
    check(await run('document.querySelector("#issuespanel .crumb").textContent.includes(' + JSON.stringify(repo) + ')'), 'with its repository named');
    console.log('   its sub-issues ' + (await run('Array.isArray(I.detail.subs)') ? 'were read' : 'could not be read with this token, and are left out'));
    await shot('7-far-issue');
    // What changes an issue is asked of GitHub in words it understands: a
    // sub-issue that is not there is refused as not found, never as a
    // question GitHub cannot read
    await run(`issuesAsk("sub_remove", Object.assign({kind: "issue", sub: "I_kwDOnotanissue"}, issueAt(I.detail))); true`);
    await until(() => run('I.bad && !!I.said'), 'GitHub\'s refusal', 30000).catch(() => {});
    const said = await run('I.said');
    console.log('   GitHub said: ' + said);
    check(!!said && !/doesn't exist on type|isn't a defined|Variable \$|Expected|Parse error/i.test(said), 'a change to its sub-issues is a question GitHub can read');
    check(await press('#issuespanel .bar button.quiet'), 'and the way back to the board is pressed');
    await until(() => run('I.view === "list" && !I.detail'), 'the board again', 15000);
    await run('I.said = ""; I.bad = false; issuesSig = ""; drawIssues(); true');
  }

  // A board made from the list of them, copied, and both deleted
  const owner = await run('I.gh.data.owner');
  const address = await run('I.gh.ref');
  await run(`document.activeElement && document.activeElement.blur(); I.gh.picking = true; I.gh.newp = {title: ${JSON.stringify(stamp + ' board')}, owner: ${JSON.stringify(owner)}, link: false}; issuesSig = ""; drawIssues(); true`);
  check(await pressText(await t('ghp.new.go')), 'a new board named and made');
  check(await until(() => run(`!!I.gh.data && I.gh.data.title === ${JSON.stringify(stamp + ' board')}`), 'the new board', 90000).catch(() => false), 'the tab is on the new board');
  const newId = await run('I.gh.data && I.gh.data.id');
  const gone = async (id) => { try { return (await gq('query($id: ID!) { node(id: $id) { ... on ProjectV2 { id } } }', { id })).node === null; } catch (e) { return true; } };
  if (newId && newId !== projectId) {
    await page('about');
    await until(() => run('!!I.gh.about'), 'the new board\'s page', 60000);
    await run(`I.gh.copy = {title: ${JSON.stringify(stamp + ' copy')}, owner: ${JSON.stringify(owner)}, drafts: false}; issuesSig = ""; drawIssues(); true`);
    check(await pressText(await t('ghp.copy.go')), 'the board copied');
    check(await until(() => run(`!!I.gh.data && I.gh.data.title === ${JSON.stringify(stamp + ' copy')}`), 'the copy', 120000).catch(() => false), 'the tab is on the copy');
    const copyId = await run('I.gh.data && I.gh.data.id');
    for (const [id, what] of [[copyId, 'the copy'], [newId, 'the new board']]) {
      if (!id || id === projectId) { check(false, what + ' to delete'); continue; }
      if (await run('I.gh.data && I.gh.data.id') !== id) {
        await run(`ghpPick((I.gh.list || []).concat([]).length ? ${JSON.stringify('')} : ""); true`);
        const url = (await gq('query($id: ID!) { node(id: $id) { ... on ProjectV2 { url } } }', { id })).node.url;
        await run(`ghpPick(${JSON.stringify(url)}); true`);
        await until(() => run(`!!I.gh.data && I.gh.data.id === ${JSON.stringify(id)}`), what, 90000);
      }
      await page('about');
      await until(() => run('!!I.gh.about && I.gh.about.id === ' + JSON.stringify(id)), what + '\'s page', 60000);
      await run('I.gh.del = true; issuesSig = ""; drawIssues(); true');
      check(await press('#issuespanel .sect .bwarn button.bad'), what + ' deleted after a yes');
      await until(() => run('!I.gh.ref && !I.gh.making'), 'no board chosen', 60000).catch(() => {});
      check(await eventually(async () => await gone(id)), 'GitHub no longer has ' + what);
    }
  }
  await run(`ghpPick(${JSON.stringify(address)}); true`);
  await until(() => run(`!!I.gh.data && I.gh.data.id === ${JSON.stringify(projectId)}`), 'the board the check began on', 90000);
  const after = await boardShape(projectId);
  check(after.fields.nodes.map((f) => f.id).join() === before.fields.nodes.map((f) => f.id).join()
    && after.views.nodes.map((w) => w.id + w.name + w.layout + (w.filter || '')).join() === before.views.nodes.map((w) => w.id + w.name + w.layout + (w.filter || '')).join(),
    'the board has the fields and views it began with');
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
    const drawn = await run('JSON.stringify({cards: document.querySelectorAll("#issuespanel .ghcard").length, rows: document.querySelectorAll("#issuespanel .ght tbody tr:not(.grp):not(.addrow)").length, cols: document.querySelectorAll("#issuespanel .ghcol").length})');
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
      if (CARDS) await everyCardAct(run, shot, read.d.views);
      if (BOARD) await everyBoardAct(run, shot, read.d.views);
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
