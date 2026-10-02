-- A tab is who it is by its uid (config TabConfig::uid), never by its name: a
-- name goes back in the bag when its tab closes, and the next tab to draw it
-- was handed the jobs the first one led, the task it was on, and its mail.
-- From here on jobs.lead, assignments.tab, opened_tabs.tab and the tab:
-- mailboxes hold uids; who sent mail, and who made a decision, keep the name
-- they had then.
--
-- Rows written before this hold names. Which tab a name meant is answered
-- from the settings, which SQL cannot read: the app rewrites them once, when
-- it starts (Store::adopt_uids), and says so in meta.

-- What each tab is called, by uid: what a uid is shown as to the AIs and the
-- person, kept after the tab closes so a job's history still reads with it
CREATE TABLE tab_names (
  uid TEXT PRIMARY KEY,
  -- the tab's id, what <@ID> names
  name TEXT NOT NULL,
  -- when it was last seen called that
  seen_at INTEGER NOT NULL
);
