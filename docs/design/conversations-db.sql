-- The record of conversations (conversations.db), as the steps in
-- crates/core/src/convo/migrations/ leave it at version 5.
--
-- Written by a test; do not edit. Change the tables by adding a step (see
-- conversations-db.md), then write this again:
--     SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core conversations_db

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

CREATE TABLE asks (
  -- One ask_tab: a tab (or a person, or a script) asking another AI tab and
  -- waiting for its reply. Unlike a send, the words are kept: the panel that
  -- shows AIs conferring puts a whole desk's asks side by side, many tabs and
  -- machines at once, and reading each one back out of the CLI's own record
  -- would be a round trip per bubble to every machine involved. They are the
  -- words this app itself wrote and read, and go when the rest of the record
  -- goes (KEEP_MS)
  id INTEGER PRIMARY KEY,
  desk TEXT NOT NULL,                 -- the desk it was asked on, by id: a tab's id is unique only there
  caller TEXT,                        -- the asking tab, by id; NULL for a person or a script outside every tab
  target TEXT NOT NULL,               -- the tab asked, by id
  text TEXT NOT NULL,                 -- everything the target was sent
  reply TEXT,                         -- everything it said back, once it had
  state TEXT NOT NULL,                -- waiting / DONE / PENDING / QUESTION / BUSY / LIMIT / FAILED / EXIT / GONE (asktab's answer states)
  round INTEGER NOT NULL DEFAULT 0,   -- which round of the caller's this was
  asked_at INTEGER NOT NULL,
  answered_at INTEGER
, thread_id INTEGER REFERENCES threads(id) ON DELETE CASCADE);

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
  last_at INTEGER NOT NULL, observed_cwd TEXT
    /* NULL: an older sighting, or a tab whose local folder was not known. */,
  UNIQUE (tab, record_id)
);

CREATE TABLE lines (
  -- One thing said in the conference: a bubble. Short by rule (the app
  -- refuses a line over the length set in the settings), one line each
  id INTEGER PRIMARY KEY,
  desk TEXT NOT NULL,
  tab TEXT,                           -- who said it, by id; NULL for the person
  said_at INTEGER NOT NULL,
  text TEXT NOT NULL,
  ask_id INTEGER REFERENCES asks(id) ON DELETE SET NULL, -- the ask it opened or answered
  how TEXT NOT NULL                   -- ask (the asker's line) / said (the answer's line, in its own words) / auto (the answer's first sentence, taken for it) / aside (said on its own) / person (a person naming a tab) / agreed (a decision made)
, thread_id INTEGER REFERENCES threads(id) ON DELETE CASCADE, tab_uid TEXT);

CREATE TABLE reactions (
  -- A mark put on a line: by a person pressing it, or by an AI
  id INTEGER PRIMARY KEY,
  line_id INTEGER NOT NULL REFERENCES lines(id) ON DELETE CASCADE,
  by TEXT NOT NULL,                   -- the tab, by id, or 'person'
  mark TEXT NOT NULL,                 -- one of the few the app offers (confer::MARKS)
  marked_at INTEGER NOT NULL,
  UNIQUE (line_id, by, mark)
);

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

CREATE TABLE shares (
  -- Something an AI put in front of the others as a card: a commit, a pull
  -- request, a file, a page. Checked before it was taken (a commit that is
  -- not in the tab's folder is refused), so a card always points at something
  id INTEGER PRIMARY KEY,
  desk TEXT NOT NULL,
  tab TEXT NOT NULL,                  -- who shared it, by id
  shared_at INTEGER NOT NULL,
  kind TEXT NOT NULL,                 -- commit / pr / file / url
  target TEXT NOT NULL,               -- the hash, the address, or the path, as it is opened
  title TEXT NOT NULL,                -- what the card says it is: a commit's subject, a file's name, the title the AI gave
  detail TEXT NOT NULL DEFAULT '{}'   -- JSON: what else the card shows (short hash, branch, folder, host)
, thread_id INTEGER REFERENCES threads(id) ON DELETE CASCADE);

CREATE TABLE spans (
  -- A stretch of time a tab spent in one state. Written when the state
  -- changes, never on a clock, so a tab waiting an hour is one row
  id INTEGER PRIMARY KEY,
  tab TEXT NOT NULL,
  state TEXT NOT NULL,                -- BUSY / QUESTION / DONE / LIMIT / ... (detect::TabState labels)
  started_at INTEGER NOT NULL,
  ended_at INTEGER                    -- NULL while the tab is still in it
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

CREATE TABLE tab_names (
  -- What a tab is called and on which desk, by its uid: what a uid is shown
  -- as, and what a name said in a conversation (`<@calm-otter>`) is taken to
  -- mean -- the tab last seen called that on that desk. Kept after the tab
  -- closes, so what it took part in still reads with its name
  uid TEXT PRIMARY KEY,
  desk TEXT NOT NULL,                 -- the desk, by id ('' when not known)
  name TEXT NOT NULL,                 -- the tab's id, what <@ID> names
  seen_at INTEGER NOT NULL            -- when it was last seen called that
);

CREATE TABLE thread_tabs (
  -- Who takes part in a conversation: named in it by a person, asked or
  -- asking in it, saying, sharing or marking something in it. Written by the
  -- same writes that record those, so it cannot fall behind them
  thread_id INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  tab TEXT NOT NULL,                  -- the tab, by id
  joined_at INTEGER NOT NULL,
  PRIMARY KEY (thread_id, tab)
);

CREATE TABLE threads (
  -- One conversation of AIs conferring. Everything asked, said and shared in
  -- it carries its id, so two conversations on one desk at once read apart
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  desk TEXT NOT NULL,
  origin TEXT NOT NULL,               -- whose conversation it grew from: the tab that began it and its CLI's conversation id, "tab/record"
  merged_into INTEGER REFERENCES threads(id) ON DELETE SET NULL, -- the conversation it was found to belong to (by the deciding AI); its rows moved there
  begun_at INTEGER NOT NULL,
  last_at INTEGER NOT NULL            -- when anything was last said in it
);

CREATE INDEX asks_by_desk ON asks (desk, asked_at);

CREATE INDEX asks_by_thread ON asks (thread_id);

CREATE INDEX conversations_by_tab ON conversations (tab, first_at);

CREATE INDEX lines_by_desk ON lines (desk, said_at);

CREATE INDEX lines_by_tab_uid ON lines (desk, tab_uid, said_at);

CREATE INDEX lines_by_thread ON lines (thread_id, said_at);

CREATE UNIQUE INDEX one_open_span ON spans (tab) WHERE ended_at IS NULL;

CREATE INDEX sends_by_tab ON sends (tab, sent_at);

CREATE INDEX shares_by_desk ON shares (desk, shared_at);

CREATE INDEX shares_by_thread ON shares (thread_id, shared_at);

CREATE INDEX spans_by_tab ON spans (tab, started_at);

CREATE INDEX stops_by_tab ON stops (tab, stopped_at);

CREATE INDEX tab_names_by_name ON tab_names (desk, name, seen_at);

CREATE INDEX thread_tabs_by_tab ON thread_tabs (tab, thread_id);

CREATE INDEX threads_by_desk ON threads (desk, last_at);

CREATE INDEX threads_by_origin ON threads (desk, origin);
