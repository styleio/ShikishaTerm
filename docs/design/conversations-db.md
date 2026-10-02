# The record of conversations (conversations.db)

> 日本語版: [conversations-db.ja.md](conversations-db.ja.md).
> The tables as they are now: [conversations-db.sql](conversations-db.sql) (written by a test; do not edit it by hand).

An AI CLI writes down every word of a conversation in a record of its own (a JSONL file), and nothing about how the words got there. Whatever this app typed into the tab -- what the person sent from the window or from a phone, what another tab asked, a job's brief -- reads as "the user" in that record. The record cannot say who answered a question the CLI asked, who stopped it, or that the tab moved to a new conversation after a `/clear`.

This app can, because all of it went through the app. It writes those things down in an SQLite file, the `conversations.db` in the app's state folder. The column's Chat panel (`convo::read`) reads the words from the CLI's record and puts these facts beside them. The code is in `crates/core/src/convo/`.

**The words of a conversation are not kept here.** They are in the CLI's record already; a second copy would be one more thing to drift and one more place a pasted token could leak from. A send keeps only fingerprints of how its text begins (the first 60 characters with runs of space made single, hashed), which is what finds it in the record again.

The conference tables (`asks`, `lines`, `reactions`, `shares`) are the exception: AIConfer puts many tabs, on many machines, side by side, and reading every bubble back out of each CLI's own record would be a round trip per bubble. What they keep is what the app itself wrote and read, and it goes when everything else here goes.

## What each table is for

| Table | One row is | Why it is kept |
|---|---|---|
| `conversations` | One conversation a tab carried on: the CLI's id for it, when the app first and last saw the tab on it, and whether the tab runs without asking first | So a tab's conversations read as one thread across a `/clear` or a conversation picked back up, and so a conversation where no questions could be asked says so |
| `sends` | One piece of text the app put into a tab: who sent it (a person, another tab, a job, automation), from where (this PC or remote), by which way, and the fingerprints of how it begins | In the CLI's record every one of these reads as the person's words |
| `spans` | A stretch of time a tab spent in one state. Written when the state changes, never on a clock | To say how long a question waited for an answer |
| `answers` | What ended a wait for an answer: the first input that reached the tab while it was asking | The CLI records what was chosen, not who chose it |
| `stops` | One stop: who, from where, how (the stop button, Esc, a job stopped, a usage limit) and what was said about it | The CLI records only that its turn was cut short |
| `asks` | One `ask_tab`: which tab (or person) asked which, everything it was sent, everything it said back, how it ended | The AIConfer view shows a whole desk's asks side by side |
| `lines` | One bubble of AIConfer: who said it, the short line, the ask it belongs to, and how it came to be (the asker's line, the answer in its own words, the answer's first sentence taken for it, a line said on its own, a person naming a tab, a decision made) | No CLI record has these: they are said to the app, not in the conversation |
| `reactions` | One mark on a line, and who put it there (a tab, or the person) | |
| `shares` | One card: a commit, a pull request, a file or a page an AI put in front of the others, checked before it was taken | |
| `threads` | One conversation of AIs: begun by a person naming a tab, or by an AI asking another, and keyed by the CLI conversation of the tab that began it. Every ask, line and card carries its id | So two conversations on one desk at once read apart |
| `thread_tabs` | Who takes part in a conversation: named in it, asking or asked, saying, sharing or marking something | AIConfer shows the conversations of the tab in front. Written by the same writes that record those, so it cannot fall behind them |
| `tab_names` | What each tab is called, and on which desk, by its uid; kept after the tab closes | So what a closed tab took part in still reads with its name, and so a name said in a conversation means the tab called that now |

`meta` holds `schema`, the version the file is at, and `tab_uids` once the rows an older version wrote under tabs' names have been rewritten under their uids.

## A tab is its uid

Every tab has a uid of its own (`config::TabConfig::uid`), drawn when the tab is made and never handed to another tab. Its name -- what `<@ID>` says -- goes back in the bag when the tab closes, and the next tab to draw it used to be handed everything kept under it: the conversation panel of a new tab showed a closed tab's conversations.

- **The columns that say which tab a row is about hold its uid**: `conversations.tab`, `sends.tab`, `spans.tab`, `stops.tab`, `thread_tabs.tab`, the tab half of `threads.origin`, `lines.tab_uid`.
- **The columns that say who said or sent something keep the name it had then**, the way a chat keeps the name a message was signed with: `sends.sender`, `asks.caller` and `target`, `lines.tab`, `shares.tab`, `reactions.by`.
- A name said in the conference is taken to mean the tab last seen called that on its desk (`Store::uid_named`). A name nobody answers to any more stands for a uid of its own (`db::gone_uid`), the same each time.
- Rows written before uids are rewritten once, on the first start, from the settings (`Store::adopt_uids`): a name goes to the tab of the settings called that, on its desk where the row says which desk, and anywhere when only one desk has it. A closed tab's rows, and a name two desks share, go to a uid no tab has.

States and the words in the `by`, `device`, `via` and `how` columns are plain text with no CHECK; what they may be is in the comments on the columns and in `convo::db` (`By`, `Device`, `Origin`, `Stop`).

## Keeping it small

- Everything older than 90 days (`db::KEEP_MS`) is deleted when the file is opened. Every CLI forgets its own records well before that (Claude Code after 30 days unless told otherwise).
- A conversation whose record is gone from this PC is forgotten, with what was sent in it, the first time the panel looks for it.
- The file is not `VACUUM`ed on start; deleted rows' space is reused.
- A row is a few hundred bytes. A thousand sends a day is about 300 KB a day.

## Who writes and who reads

- Only the app's main loop writes, through `convo::Log`. What happens far from it -- a job typing a brief, a script's line, Esc pressed at the window -- is noted where it happens (`convo::note_sent`, `note_touched`, `note_stopped`) and written at the loop's next look.
- The panel reads on a thread of its own, over a read-only connection (`db::Store::open_read`), which WAL lets run beside the writer.
- For the search of every conversation: `Store::chain_of(record_id)` gives the conversations one tab carried on, `read::origins(record_id, lines)` says who sent each of a person's lines, and `marks::all` gives the pins and notes.

## Pins and notes are not here

What a person writes about a conversation -- a pin, a note -- is theirs, and is kept with their settings in `config/conversation-marks.json`, as the ideas are. Not in SQLite: the settings folder can be one another PC syncs, and a database with a write-ahead log beside it is what such a folder breaks.

## Changing the tables

The same as every SQLite file of this app ([sqlite.md](sqlite.md)): add `crates/core/src/convo/migrations/NNNN_what.sql` and a line to `STEPS` in `convo/db.rs`, never edit a shipped step, then write the design file again and read its diff:

```
SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core conversations_db
```

The tests of section 8 of sqlite.md are in `convo/db.rs`.

## Checking it in the running app

`tools/debug/convo-panel.win.mjs` starts this checkout's build in a folder of its own with a stand-in Claude and a shell, and checks, through the window and the board's remote door: a line from the window, a line from a remote page, a line another tab sent, a question answered from afar, Esc at the window, the boxes, the search (a word only in a tool's output), a pin, and that the file holds none of the words. Pictures: `node tools/debug/shoot.mjs tools/debug/scenes/convo.mjs`.
