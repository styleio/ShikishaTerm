-- The record of tokens used (data/tokens.db), version 2.
-- Written by a test from the steps in crates/core/src/tokens/migrations;
-- what each table is for is in tokens-db.md. Write it again with
--     SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core tokens_db

CREATE TABLE calls (
  -- One call to a model, as its CLI wrote it down: what went in and what
  -- came out. Kept after the CLI forgets its own record (Claude Code does
  -- after 30 days), and given up after DAYS_KEPT
  key TEXT PRIMARY KEY,          -- the same call written twice (while it streamed, or copied into a resumed conversation) is one row
  cli TEXT NOT NULL,             -- claude, codex
  at INTEGER NOT NULL,           -- when it was made
  day TEXT NOT NULL,             -- the date it was made on, by this PC's clock when it was read (YYYY-MM-DD)
  model TEXT NOT NULL DEFAULT '',
  folder TEXT NOT NULL DEFAULT '',  -- the folder the CLI was working in
  session TEXT NOT NULL DEFAULT '', -- the conversation it belongs to
  fresh_in INTEGER NOT NULL DEFAULT 0,         -- input tokens not read from the cache
  cache_read INTEGER NOT NULL DEFAULT 0,       -- input tokens read from the cache
  cache_write INTEGER NOT NULL DEFAULT 0,      -- input tokens written to the cache for five minutes
  cache_write_hour INTEGER NOT NULL DEFAULT 0, -- input tokens written to the cache for an hour
  out INTEGER NOT NULL DEFAULT 0,              -- output tokens, reasoning included
  reasoning INTEGER NOT NULL DEFAULT 0,        -- the part of out that was reasoning, where the CLI says
  is_fast INTEGER NOT NULL DEFAULT 0,          -- made in Claude's fast mode, which is billed at a higher rate
  is_us INTEGER NOT NULL DEFAULT 0             -- kept to the United States (Claude's inference_geo), billed at a higher rate
);

CREATE TABLE read_files (
  -- One conversation record a CLI wrote on this PC, and how far it has been
  -- read: the next look reads only what was added since
  path TEXT PRIMARY KEY,         -- the record, as it was found
  cli TEXT NOT NULL,             -- whose record it is: claude, codex
  size INTEGER NOT NULL,         -- its length in bytes when it was last read
  read_to INTEGER NOT NULL,      -- bytes read, up to the end of its last whole line
  head TEXT NOT NULL,            -- a hash of its first bytes: a file written anew under the same name is read from the start
  edge TEXT NOT NULL,            -- a hash of the bytes just before read_to: the same check where reading goes on
  carry TEXT NOT NULL DEFAULT '{}', -- what later lines need from earlier ones (Codex: its model, folder, conversation and running total), as JSON
  read_at INTEGER NOT NULL       -- when it was last read
);

CREATE INDEX calls_by_day ON calls(day);

CREATE INDEX calls_by_session ON calls(session);
