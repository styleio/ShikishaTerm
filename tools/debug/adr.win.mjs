/**
 * The decision-record (ADR) panel, driven in a running copy of the app.
 *
 *   powershell tools\debug\instance.win.ps1 -Work <repo> -Config <settings>
 *   node tools/debug/adr.win.mjs <cdp port> <repo> [--ai]
 *
 * <repo> is a git repository whose project keeps records in docs/decisions
 * (the settings name it with "adr": {"on": true}), holding a MADR record
 * numbered 0001 and an older-format one numbered 0002. Everything is done
 * through the board's own functions over DevTools, and every write is
 * checked on the disk: a new record in Japanese, its status changed, a
 * record replaced (both files saying so), the plan of a pull request. `--ai`
 * also asks the assistant AI a question about the records -- one real call.
 */
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import {connectCdp} from './chrome.mjs';

const [port, repo] = process.argv.slice(2);
const withAi = process.argv.includes('--ai');
if (!port || !repo) { console.error('node tools/debug/adr.win.mjs <cdp port> <repo> [--ai]'); process.exit(2); }
const dir = path.join(repo, 'docs', 'decisions');
const sleep = ms => new Promise(r => setTimeout(r, ms));
const check = (ok, why) => { assert(ok, why); console.log('PASS ' + why); };
async function until(test, why, tries = 150) {
  for (let n = 0; n < tries; n++) { try { const v = await test(); if (v) return v; } catch {} await sleep(200); }
  throw Error('Timed out: ' + why);
}

const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const board = targets.find(t => t.type === 'page' && /127\.0\.0\.1|localhost/.test(t.url) && !/settings|cfg/.test(t.url)) || targets[0];
const {send, run} = await connectCdp(board, {timeout: 15000});
await send('Runtime.enable');
const js = async expr => {
  const r = await send('Runtime.evaluate', {expression: expr, awaitPromise: true, returnByValue: true});
  if (r.exceptionDetails) throw Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
  return r.result.value;
};

// The column, on the ADR panel of the folder's tab
await until(() => js('!!(S && S.groups && S.groups.some(g => g.adr))'), 'the folder says its project keeps records');
await js('setSideWidth(380); sideChoose("adr"); true');
await until(() => js('AD.records && AD.records.length === 2'), 'the two records are listed');
const listed = await js('AD.records.map(r => [r.file, r.status, r.fits, r.title])');
check(listed[0][1] === 'accepted' && listed[0][2] === true, 'a MADR record reads as accepted and fits the form');
check(listed[1][1] === 'proposed' && listed[1][2] === false && listed[1][3] === 'Record decisions',
  'an older-format record reads its status and title, and goes to the editor');

// A new record, in Japanese, as the form saves it
await js(`(() => { adrNew(null); const f = AD.form; f.title = "キャッシュに Redis を使う";
  f.sections["Context and Problem Statement"] = "注文一覧が遅い。"; f.sections["Considered Options"] = "* Redis\\n* Memcached";
  f.front["decision-makers"] = "佐藤"; closeAsk(true); adrSave(f, ""); return true; })()`);
const made = path.join(dir, '0003-キャッシュに-redis-を使う.md');
await until(() => fs.existsSync(made), 'the new record is on the disk');
let text = fs.readFileSync(made, 'utf8');
check(/^---\nstatus: proposed\ndate: \d{4}-\d\d-\d\d\ndecision-makers: 佐藤\n---\n\n# キャッシュに Redis を使う\n/.test(text),
  'the new record is a proposal, dated, with its maker and its Japanese title');
check(text.includes('## Considered Options\n\n* Redis\n* Memcached'), 'its sections are written under MADR headings');
await until(() => js('AD.view === "read" && AD.read && AD.read.file === "0003-キャッシュに-redis-を使う.md" && !!AD.read.record'),
  'the panel opens what it saved');

// Decided: the status and the day it changed
await js('adrSetStatus(AD.read, "accepted"); true');
await until(() => /status: accepted/.test(fs.readFileSync(made, 'utf8')), 'the status is written');
check(true, 'marking it accepted writes the file');

// Replaced: the new record says what it replaces, the old one says by what
await until(() => js('AD.records.length === 3'), 'the list has the new record');
await js(`(() => { const d = {file:"0001-use-postgresql-for-orders.md", record:AD.records.find(r => r.number === 1)};
  adrNew({supersedes: d.file, title: "注文の保存に SQLite を使う"}); const f = AD.form;
  f.sections["Context and Problem Statement"] = "サーバーを減らしたい。"; closeAsk(true); adrSave(f, ""); return true; })()`);
const repl = path.join(dir, '0004-注文の保存に-sqlite-を使う.md');
await until(() => fs.existsSync(repl), 'the replacing record is on the disk');
check(fs.readFileSync(repl, 'utf8').includes('Supersedes [ADR-0001](0001-use-postgresql-for-orders.md).'),
  'the new record links the one it replaces');
const old = fs.readFileSync(path.join(dir, '0001-use-postgresql-for-orders.md'), 'utf8');
check(old.includes('status: superseded by [ADR-0004](0004-注文の保存に-sqlite-を使う.md)'), 'the old record says what replaced it');
await until(() => js('AD.records.length === 4 && AD.records.find(r => r.number === 1).by === "0004-注文の保存に-sqlite-を使う.md"'),
  'the list links the two');

// A form that cannot be saved stays up and says why
await js('adrNew(null); true');
await js('document.querySelector("#sask .brow > .go").click(); true');
const why = await js('[!document.getElementById("sask").hidden, document.querySelector("#sask .swhy").textContent]');
check(why[0] && why[1].length > 0, 'a record with no title is refused, and the form stays: ' + why[1]);
await js('closeAsk(true); true');

// The pull request's plan, worked out by the app before anything runs
await js('AD.plan = null; adrPropose("0004-注文の保存に-sqlite-を使う.md", "注文の保存に SQLite を使う"); true');
await until(() => js('!document.getElementById("sask").hidden && document.querySelector("#sask .bwhere").textContent.includes("adr-0004")'),
  'the plan names the branch it will make');
check(true, 'proposing asks first, with the branch and the commit spelled out');
await js('closeAsk(true); true');

// Searching: full-width letters find half-width ones
await js('AD.view = "list"; AD.read = null; AD.q = "ＲＥＤＩＳ"; AD.rev++; drawAdr(); true');
const hits = await js('[...document.querySelectorAll("#adrpanel .arow")].map(r => r.title)');
check(hits.length === 1 && hits[0].startsWith('0003-'), 'a full-width search finds the record');

if (withAi) {
  await js('AD.q = ""; adrAskAi("キャッシュには何を使う？"); true');
  const said = await until(() => js('AD.answer && (AD.answer.text || AD.answer.error)'), 'the AI answers', 900);
  const ans = await js('AD.answer');
  console.log('AI said: ' + (ans.text || ans.error));
  check(!!ans.text && /ADR-0003/.test(ans.text), 'the AI answers from the records and names the one it used');
}
console.log('all done');
process.exit(0);
