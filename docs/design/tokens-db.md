# The record of tokens used (tokens.db)

> The tables as they are now: [tokens-db.sql](tokens-db.sql) (written by a test; do not edit it by hand).

Claude Code and Codex write down, in their own conversation records (JSONL files), how many tokens each call to a model took and gave. The settings card "Tokens and cost" adds those up: by day, by model, by folder and by conversation, and prices them by the makers' published per-token prices (`crates/core/src/tokens/price.rs`). The code is in `crates/core/src/tokens/`.

The numbers are copied into an SQLite file, `tokens.db` in the app's state folder, one row per call, for two reasons:

- **Only what was added is read again.** The records on a machine that has been used for a while run to gigabytes. Each record's row in `read_files` says how far it was read, and a record that only grew is read from there.
- **The numbers outlive the records.** Claude Code deletes its records after 30 days unless told otherwise; a year's view would be empty past that. Calls are kept for 400 days (`DAYS_KEPT`).

**No words are kept here,** only counts, the model's name, the folder and the conversation's id.

## What each table is for

| Table | One row is | Why it is kept |
|---|---|---|
| `read_files` | One record on this PC: how long it was, how far it was read (to the end of its last whole line), a hash of its first bytes and of the bytes just before that point, and what later lines need from earlier ones (`carry`) | A record that only grew is read from where it was left. A record written anew under the same name (the hashes differ) is read from the start. A Codex record names its model and folder once, on lines before the counts, so those are carried from one reading to the next |
| `calls` | One call to a model: when, the model, the folder, the conversation, and each kind of token (fresh input, read from the cache, written to the cache for five minutes and for an hour, output, the reasoning part of output), and whether it ran in Claude's fast mode or was kept to the United States | Priced when the card asks, so a price corrected in a later version applies to every call already counted |

## The same call is one row

- Claude Code writes an assistant message again while it streams, with larger numbers, under the same message id and request id. The key is `c:<message id>:<request id>`, and the larger of two sightings is kept whole -- its kinds of token and how it was billed together, not the largest of each from two readings.
- A conversation resumed or forked copies its earlier lines into a new record. The copies have the same keys, so they are the same rows.
- Codex gives a call no id. Its key is the time it was counted and the four numbers counted (`x:<time>:<input>:<cached>:<output>:<reasoning>`): the same wherever the line is copied to. Its conversation id is left out, since a resumed conversation is written under another one.
- Codex says its count again, under a new time, when only what is left of the plan changed. A count whose running total has not moved is not a call. (The first version counted these; step 2 had Codex's records read again.)
- Codex's records are in `sessions`, and in `archived_sessions` once a conversation is archived; both are read.

## Prices

A call is priced by its model, at the rate for its prompt's length (Claude Haiku 5.5 over 100,000 tokens, OpenAI's models over 272,000), doubled in Claude's fast mode for the models that have it, and multiplied by 1.1 when kept to the United States. The report groups calls finely enough that every group has one price. A model that is not in the price list is counted and not priced; the card says how many calls that leaves out.

## Adding a step

Write `crates/core/src/tokens/migrations/000N_<what>.sql`, add it to `STEPS`, and write the design file again:

    SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core tokens_db

The rules for steps are in [sqlite.md](sqlite.md).
