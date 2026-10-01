-- AIConfer before conversations had ids was never released: what testing
-- left in its tables goes, rather than being guessed into conversations
DELETE FROM reactions;
DELETE FROM lines;
DELETE FROM shares;
DELETE FROM asks;

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

CREATE INDEX threads_by_desk ON threads (desk, last_at);
CREATE INDEX threads_by_origin ON threads (desk, origin);

CREATE TABLE thread_tabs (
  -- Who takes part in a conversation: named in it by a person, asked or
  -- asking in it, saying, sharing or marking something in it. Written by the
  -- same writes that record those, so it cannot fall behind them
  thread_id INTEGER NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  tab TEXT NOT NULL,                  -- the tab, by id
  joined_at INTEGER NOT NULL,
  PRIMARY KEY (thread_id, tab)
);

CREATE INDEX thread_tabs_by_tab ON thread_tabs (tab, thread_id);

ALTER TABLE asks ADD COLUMN thread_id INTEGER REFERENCES threads(id) ON DELETE CASCADE;
ALTER TABLE lines ADD COLUMN thread_id INTEGER REFERENCES threads(id) ON DELETE CASCADE;
ALTER TABLE shares ADD COLUMN thread_id INTEGER REFERENCES threads(id) ON DELETE CASCADE;

CREATE INDEX asks_by_thread ON asks (thread_id);
CREATE INDEX lines_by_thread ON lines (thread_id, said_at);
CREATE INDEX shares_by_thread ON shares (thread_id, shared_at);
