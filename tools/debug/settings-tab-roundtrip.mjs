/**
 * A tab's line through the settings screen's save, with no browser: the
 * screen's own `flatten` (reading a line into its fields) and `nest` (writing
 * them back), taken out of crates/core/src/webui.rs as they are and run here.
 *
 *     node tools/debug/settings-tab-roundtrip.mjs
 *
 * Checked: what the screen does not show -- a split's panes, a conversation to
 * resume, a key a later version adds -- comes back as it went in; "start clean"
 * and the git account, which the screen shows, are read in and written back;
 * an edit is saved; children are rebuilt once; nothing of the screen's own
 * (`rest`, `depth`) reaches the file. Written after saving the settings was
 * found to drop every key the screen did not know. A command written as its
 * words apart (a program under C:\Program Files, such as Git Bash) is saved
 * as it was, and the command field's line turns into it and back.
 *
 * Needs Node. Reads nothing but webui.rs, writes one file in the temp folder.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const here = path.dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1'));
const src = fs.readFileSync(path.join(here, '..', '..', 'crates', 'core', 'src', 'webui.rs'), 'utf8');
const from = src.indexOf('const TAB_KEYS_SHOWN');
const to = src.indexOf('async function loadAi()');
if (from < 0 || to < from) throw new Error('flatten and nest are not where they were in webui.rs');
const words = src.match(/const WORDS_KEYS = [^;]*;/)[0];
const nav = src.match(/const NAV_PARTS = [^;]*;/)[0];
const cmdFrom = src.indexOf('const cmdToText = c =>');
const cmdTo = src.indexOf('// The program a command runs', cmdFrom);
if (cmdFrom < 0 || cmdTo < cmdFrom) throw new Error('cmdToText and textToCmd are not where they were in webui.rs');
const code = `${words}\n${nav}\n` + src.slice(cmdFrom, cmdTo) + src.slice(from, to) + '\nexport { flatten, nest, cmdToText, textToCmd };';
const file = path.join(os.tmpdir(), `settings-tab-roundtrip-${process.pid}.mjs`);
fs.writeFileSync(file, code);
const { flatten, nest, cmdToText, textToCmd } = await import(pathToFileURL(file).href);
fs.rmSync(file, { force: true });
const tabs = [
  { name: "Split", id: "sawfly", uid: "u1", command: "split", panes: { layout: { root: 1 }, keys: [[1, "u2"]] } },
  { name: "Claude", id: "calm-otter", uid: "u2", command: "claude", restore_conversation: false, git_account: "@pc", resume: "abc",
    future_key: { x: 1 }, children: [ { name: "Kid", id: "kid", uid: "u3", command: "codex", locked: true } ] },
];
const flat = flatten(tabs, 0, 0, []);
flat[1].name = "Renamed";
const back = nest(flat);
const ok = (c, m) => { console.log((c ? 'PASS ' : 'FAIL ') + m); if (!c) process.exitCode = 1; };
ok(JSON.stringify(back[0].panes) === JSON.stringify(tabs[0].panes), 'a split keeps its panes');
ok(back[1].restore_conversation === false, 'start clean is kept');
ok(back[1].git_account === '@pc', 'git account is kept');
ok(back[1].resume === 'abc' && back[1].future_key.x === 1, 'keys the screen never shows are kept');
ok(back[1].name === 'Renamed', 'an edit is saved');
ok(back[1].children.length === 1 && back[1].children[0].locked === true && back[1].children[0].uid === 'u3', 'children are rebuilt, not copied twice');
ok(!('rest' in back[1]) && !('depth' in back[1]), 'nothing of the screen leaks into the file');
flat[1].restore_conversation = undefined; delete flat[1].restore_conversation;
ok(!('restore_conversation' in nest(flat)[1]), 'turned back on, the key goes');
// A program under a path with spaces is written as the words apart
// (config::command_value); read and saved, it is not cut at its space
const bash = ['C:\\Program Files\\Git\\bin\\bash.exe', '--login', '-i'];
const kept = nest(flatten([{ name: 'Git Bash', id: 'git-bash', uid: 'u4', command: bash }], 0, 0, []));
ok(JSON.stringify(kept[0].command) === JSON.stringify(bash), 'a command written apart is saved apart: ' + JSON.stringify(kept[0].command));
const line = cmdToText(bash);
ok(line === '"C:\\Program Files\\Git\\bin\\bash.exe" --login -i', 'the field shows it as one line, its program quoted: ' + line);
ok(JSON.stringify(textToCmd(line)) === JSON.stringify(bash), 'and that line is written back apart');
ok(textToCmd('claude --model opus') === 'claude --model opus', 'a line with no quoted program stays a line');
console.log(process.exitCode ? 'some failed' : 'all passed');
