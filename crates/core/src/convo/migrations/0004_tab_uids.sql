-- A tab is who it is by its uid (config TabConfig::uid), never by its name: a
-- name goes back in the bag when its tab closes, and the next tab to draw it
-- was handed every conversation, wait and stop kept under it. From here on
-- the columns that say which tab a row is about hold its uid --
-- conversations.tab, sends.tab, spans.tab, stops.tab, thread_tabs.tab, the
-- tab half of threads.origin and lines.tab_uid. The ones that say who said or
-- sent something keep the name it had then, as a chat keeps the name a
-- message was signed with: sends.sender, asks.caller and target, lines.tab,
-- shares.tab, reactions.by.
--
-- Rows written before this hold names. Which tab a name meant can only be
-- answered from the settings, which SQL cannot read: the main loop rewrites
-- them once, on its first look (Store::adopt_uids), and says so in meta.

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

CREATE INDEX tab_names_by_name ON tab_names (desk, name, seen_at);

ALTER TABLE lines ADD COLUMN tab_uid TEXT; -- who said it, by uid; NULL for the person

CREATE INDEX lines_by_tab_uid ON lines (desk, tab_uid, said_at);
