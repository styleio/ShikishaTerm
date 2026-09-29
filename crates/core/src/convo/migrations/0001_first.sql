CREATE TABLE conversations (
  -- One conversation a tab carried on: the CLI's own conversation id, and
  -- when this app first and last saw the tab on it. A tab moves to a new one
  -- when the CLI starts afresh (/clear) or picks another up; read in order,
  -- these are everything said in that tab
  id INTEGER PRIMARY KEY,
  tab TEXT NOT NULL,                  -- the tab's id (what <@ID> names)
  cli TEXT NOT NULL,                  -- which CLI: claude, codex, gemini ...
  record_id TEXT NOT NULL,            -- the CLI's id for the conversation
  is_yolo INTEGER NOT NULL DEFAULT 0, -- 1 when the tab ran without asking first (no questions to answer)
  first_at INTEGER NOT NULL,
  last_at INTEGER NOT NULL,
  UNIQUE (tab, record_id)
);

CREATE INDEX conversations_by_tab ON conversations (tab, first_at);

CREATE TABLE sends (
  -- One piece of text this app put into a tab: who sent it, from where, by
  -- which way. In the CLI's record every one of these reads as the person's
  -- words; this says whose they were. The words themselves are not kept
  id INTEGER PRIMARY KEY,
  tab TEXT NOT NULL,                  -- the tab it went into, by id
  record_id TEXT,                     -- the conversation the tab was on, when known
  sent_at INTEGER NOT NULL,
  by TEXT NOT NULL,                   -- person / tab / job / automation
  device TEXT,                        -- window / phone, for a person; NULL when not known
  via TEXT NOT NULL,                  -- composer / reply / quick / ask / script / brief / mail
  sender TEXT,                        -- the tab that sent it (by tab), or the job's lead (by job)
  job INTEGER,                        -- the job's number in orchestration.db (by job)
  heads TEXT NOT NULL DEFAULT '[]',   -- JSON list: fingerprints of how the text begins, to find it in the record
  chars INTEGER NOT NULL DEFAULT 0    -- how long the text was
);

CREATE INDEX sends_by_tab ON sends (tab, sent_at);

CREATE TABLE spans (
  -- A stretch of time a tab spent in one state. Written when the state
  -- changes, never on a clock, so a tab waiting an hour is one row
  id INTEGER PRIMARY KEY,
  tab TEXT NOT NULL,
  state TEXT NOT NULL,                -- BUSY / QUESTION / DONE / LIMIT / ... (detect::TabState labels)
  started_at INTEGER NOT NULL,
  ended_at INTEGER                    -- NULL while the tab is still in it
);

CREATE INDEX spans_by_tab ON spans (tab, started_at);

CREATE UNIQUE INDEX one_open_span ON spans (tab) WHERE ended_at IS NULL;

CREATE TABLE answers (
  -- What ended a wait for an answer: the first input that reached a tab while
  -- it was asking (a permission question, a choice). The CLI records what was
  -- chosen; this records who chose
  id INTEGER PRIMARY KEY,
  span_id INTEGER NOT NULL UNIQUE REFERENCES spans(id) ON DELETE CASCADE,
  answered_at INTEGER NOT NULL,
  by TEXT NOT NULL,                   -- person / tab / job / automation
  device TEXT,                        -- window / phone, for a person
  via TEXT NOT NULL                   -- keys / composer / reply / script ...
);

CREATE TABLE stops (
  -- One time a tab was stopped, or stopped by itself at a usage limit: who,
  -- from where, how, and what was said about it. The CLI's record says only
  -- that its turn was cut short
  id INTEGER PRIMARY KEY,
  tab TEXT NOT NULL,
  stopped_at INTEGER NOT NULL,
  by TEXT NOT NULL,                   -- person / lead / limit / automation
  device TEXT,                        -- window / phone, for a person
  how TEXT NOT NULL,                  -- all (the stop button) / esc / job / limit
  job INTEGER,                        -- the job it belonged to, when a job was stopped
  why TEXT                            -- the reason as given: the limit line, the job's word
);

CREATE INDEX stops_by_tab ON stops (tab, stopped_at);
