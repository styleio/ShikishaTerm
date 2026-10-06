/**
 * Markdown in the editor, driven in a running copy of the app.
 *
 *   powershell tools\debug\instance.win.ps1 -Work <repo> -Config <settings>
 *   node tools/debug/markdown.win.mjs <cdp port> <repo> [shots folder]
 *
 * <repo> is a folder the copy has a terminal tab in. The checker writes two
 * documents into docs/ of it, opens them through the editor's own door and
 * checks, on the screen and on the disk:
 *   - a plain document opens visually; one word changed and saved changes
 *     that word only -- its `*` lists, blank lines, CRLF and its end as they were
 *   - a document the visual editor would rewrite (footnotes, HTML) opens as
 *     text and says why
 *   - the preview: front matter, headings, a table, maths, a diagram, a picture
 *     from the folder, a footnote, and no script
 *   - the contents, the search and its count, a heading reached from a link
 * Pictures of each view are kept when a shots folder is named.
 */
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import {connectCdp} from './chrome.mjs';

const [port, repo, shots] = process.argv.slice(2);
if (!port || !repo) { console.error('node tools/debug/markdown.win.mjs <cdp port> <repo> [shots folder]'); process.exit(2); }
const sleep = ms => new Promise(r => setTimeout(r, ms));
const check = (ok, why) => { assert(ok, why); console.log('PASS ' + why); };
async function until(test, why, tries = 100) {
  for (let n = 0; n < tries; n++) { try { const v = await test(); if (v) return v; } catch {} await sleep(150); }
  throw Error('Timed out: ' + why);
}

// The documents
fs.mkdirSync(path.join(repo, 'docs', 'img'), {recursive: true});
const plain = '# 手順書\r\n\r\n* 準備する\r\n* 確認する\r\n\r\n\r\n本文の単語です。\r\n\r\n| a | b |\r\n|---|---|\r\n| 1 | 2 |\r\n';
fs.writeFileSync(path.join(repo, 'docs', 'plain.md'), plain);
fs.writeFileSync(path.join(repo, 'docs', 'img', 'bar.png'), png(120, 40));
fs.writeFileSync(path.join(repo, 'docs', 'full.md'), [
  '---', 'title: ガイド', '---', '', '# ガイド', '', '1行目', '2行目', '', '## 表', '',
  '| 項目 | 値 |', '|:---|---:|', '| 速さ | 120 |', '', '## 式', '', '文中の $E=mc^2$', '', '$$', 'a^2+b^2=c^2', '$$', '',
  '## 図', '', '```mermaid', 'flowchart LR', '  A[提案] --> B[承認]', '```', '',
  '## 画像', '', '![帯](img/bar.png)', '', '[表へ](#表) と脚注[^1]。', '', '[^1]: 脚注です。', '',
  '<script>window.__mdBad = true</script>', '',
].join('\n'));

const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const board = targets.find(t => t.type === 'page' && !/settings|cfg/.test(t.url)) || targets[0];
const {send} = await connectCdp(board, {timeout: 20000});
await send('Runtime.enable');
const js = async expr => {
  const r = await send('Runtime.evaluate', {expression: expr, awaitPromise: true, returnByValue: true});
  if (r.exceptionDetails) throw Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
  return r.result.value;
};
// What this page remembers of earlier runs (the way each file was looked at)
await js('MD.view = {}; MD.why = {}; MD.force = {}; MD.toc = {}; true');
const shot = async name => {
  if (!shots) return;
  fs.mkdirSync(shots, {recursive: true});
  const s = await send('Page.captureScreenshot', {format: 'png'});
  fs.writeFileSync(path.join(shots, name + '.png'), Buffer.from(s.data, 'base64'));
};
const open = async file => {
  await js(`(() => { const t = S.tabs.find(x => x.kind === "pty"); send({kind:"editopen", panel:t.id||t.name, path:${JSON.stringify(file)}}); return true; })()`);
  await until(() => js(`ED.path === ${JSON.stringify(file)} && !ED.loading && !!window.MdKit`), 'the editor reads ' + file);
};

// A plain document: visual, one word changed, saved
await open('docs/plain.md');
await until(() => js('!!MD.rich'), 'the plain document opens visually');
check(true, 'a plain document opens in the visual editor');
await js(`(() => { const ed = MD.rich.r.editor; let at = -1;
  ed.state.doc.descendants((n, p) => { if (n.isText && n.text.includes("単語")) at = p + n.text.indexOf("単語"); });
  ed.chain().focus().insertContentAt({from: at, to: at + 2}, "言葉").run(); return true; })()`);
await until(() => js('ED.dirty'), 'typing makes it unsaved');
await shot('visual');
await js('editSave(); true');
await until(() => js('!ED.dirty'), 'it saves');
const saved = fs.readFileSync(path.join(repo, 'docs', 'plain.md'), 'utf8');
check(saved === plain.replace('単語', '言葉'), 'only the word changed: lists, blank lines, CRLF and the end are as written -- got ' + JSON.stringify(saved));

// A document the visual editor would rewrite: as text, with why
await open('docs/full.md');
await until(() => js('mdShown() === "" && edUi.say.textContent.length > 0'), 'the full document falls back to text');
check(await js('MD.view["docs/full.md"] === "source" && MD.why["docs/full.md"].kind === "changes"'),
  'a document with a footnote and HTML opens as text and says why');

// The preview
await js('mdChoose("preview"); true');
await until(() => js('mdShown() === "preview" && !!edUi.main.querySelector(".md")'), 'the preview is drawn');
await until(() => js('edUi.main.querySelectorAll(".mddiagram svg").length === 1'), 'the diagram is drawn', 200);
await until(() => js('[...edUi.main.querySelectorAll(".md img")].every(i => i.src.startsWith("blob:"))'), 'the picture is read from the folder');
const seen = JSON.parse(await js(`JSON.stringify((() => { const m = edUi.main; return {
  front: !!m.querySelector(".mdfront"), h: m.querySelectorAll(".md > h1, .md > h2").length, table: !!m.querySelector(".md table"),
  katex: m.querySelectorAll(".md .katex").length, foot: !!m.querySelector(".md .footnotes"), script: m.querySelectorAll("script").length,
  bad: !!window.__mdBad, br: m.querySelectorAll(".md p br").length, from: m.querySelector(".md h1").dataset.from }; })())`));
check(seen.front && seen.h === 5 && seen.table && seen.katex === 2 && seen.foot, 'front matter, headings, table, maths and footnote are drawn: ' + JSON.stringify(seen));
check(seen.script === 0 && !seen.bad, 'a script in the document is taken out and never runs');
check(seen.br >= 1, 'a line break in the file is a line break on the screen');
check(seen.from === '5', 'a block knows the line of the file it came from (front matter counted)');
await shot('preview');

// The contents, and a heading reached from a link
await js('mdTocToggle(); true');
await until(() => js('!edUi.tocs.hidden && edUi.tocs.querySelectorAll(".etocrow").length === 5'), 'the contents list the headings');
check(true, 'the contents list every heading');
await js('[...edUi.main.querySelectorAll(".md a")].find(a => decodeURIComponent(a.getAttribute("href")) === "#表").click(); true');
await until(() => js('document.activeElement && document.activeElement.id === "表"'), 'the link brings its heading');
check(true, 'a link to a heading brings that heading into view');
await shot('contents');

// The search
await js('mdFindOpen(false); MD.find.q = "脚注"; mdFindRun(true); true');
const found = await js('MD.find.matches.length');
check(found === 2, 'the search finds the words in the drawn document: ' + found);
await shot('search');
await js('mdFindClose(); mdTocToggle(); true');

// A note beside a paragraph: written, drawn under it, kept, deleted
await js('NT.notes.filter(n => n.path === ED.path).forEach(n => notesAsk("drop", {id: n.id})); true');
await sleep(500);
await js(`(() => { const p = [...edUi.main.querySelectorAll(".md > p")].find(x => x.textContent.includes("脚注"));
  mdNoteWrite(p, {from: Number(p.dataset.from), to: Number(p.dataset.to), quote: p.textContent.trim()});
  const a = edUi.main.querySelector(".mdnotenew textarea"); a.value = "ここを短くする";
  a.dispatchEvent(new KeyboardEvent("keydown", {key: "Enter", bubbles: true})); return true; })()`);
await until(() => js('edUi.main.querySelectorAll(".mdnote").length === 1'), 'the note is drawn under its paragraph');
const note = JSON.parse(await js('JSON.stringify(NT.notes.find(n => n.path === ED.path))'));
check(note.text === 'ここを短くする' && note.from === 35, 'a note keeps its words and the line of the file it is about: ' + JSON.stringify(note));
check(await js('!!edUi.main.querySelector(".mdnotes")'), 'the bar says how many notes there are and hands them to an AI');
await shot('notes');
await js('notesAsk("drop", {id: NT.notes.find(n => n.path === ED.path).id}); true');
await until(() => js('edUi.main.querySelectorAll(".mdnote").length === 0'), 'a deleted note goes');
check(true, 'a note is deleted');
console.log('all done');
process.exit(0);

// A picture of a blue bar, made here so the checker brings its own
function png(w, h) {
  const raw = Buffer.alloc((w * 3 + 1) * h);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) raw.set([40, 120, 220], y * (w * 3 + 1) + 1 + x * 3);
  const table = Array.from({length: 256}, (_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c >>> 0; });
  const crc = b => { let c = 0xffffffff; for (const v of b) c = table[(c ^ v) & 255] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; };
  const chunk = (t, d) => { const l = Buffer.alloc(4); l.writeUInt32BE(d.length); const td = Buffer.concat([Buffer.from(t), d]); const c = Buffer.alloc(4); c.writeUInt32BE(crc(td)); return Buffer.concat([l, td, c]); };
  const ihdr = Buffer.alloc(13); ihdr.writeUInt32BE(w, 0); ihdr.writeUInt32BE(h, 4); ihdr[8] = 8; ihdr[9] = 2;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', zlib.deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
}
