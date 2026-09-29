-- The record of work handed between AI tabs (orchestration.db), as the
-- steps in crates/core/src/orch/migrations/ leave it at version 1.
--
-- Written by a test; do not edit. Change the tables by adding a step (see
-- orchestration-db.md), then write this again:
--     SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core orchestration_db

CREATE TABLE assignments (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  job_id INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
  task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  tab TEXT NOT NULL,
  -- which run of the tab's program was given it
  process INTEGER,
  -- 1 when the job's lead is a person's tab, one more for each tab below it
  depth INTEGER NOT NULL,
  -- handing (the brief not yet taken), working, done, failed, stopped
  state TEXT NOT NULL,
  -- whether the tab was seen starting on it: 1, 0, or NULL while handing
  seen_starting INTEGER,
  -- why it ended, when it ended some other way than a report
  end_reason TEXT,
  -- what became of the tab afterwards: released, kept, reused
  afterwards TEXT,
  created_at INTEGER NOT NULL,
  ended_at INTEGER
);

CREATE TABLE decisions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  job_id INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
  task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  question TEXT NOT NULL,
  choices TEXT NOT NULL DEFAULT '[]',
  -- lead, person
  decided_by TEXT NOT NULL,
  -- open, made
  state TEXT NOT NULL,
  choice TEXT,
  made_by TEXT,
  asked_at INTEGER NOT NULL,
  made_at INTEGER
);

CREATE TABLE handovers (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  box TEXT NOT NULL,
  -- the mail it carries, as a JSON list of seq
  mail TEXT NOT NULL,
  -- given, dealt
  state TEXT NOT NULL,
  given_at INTEGER NOT NULL,
  dealt_at INTEGER
);

CREATE TABLE jobs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  goal TEXT NOT NULL,
  -- the tab that leads it, by the id it is named by
  lead TEXT NOT NULL,
  -- open, closed
  state TEXT NOT NULL,
  -- what the lead said was done, when it closed the job
  outcome TEXT,
  started_at INTEGER NOT NULL,
  closed_at INTEGER
);

CREATE TABLE made_folders (
  job_id INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
  folder TEXT NOT NULL,
  branch TEXT NOT NULL,
  made_at INTEGER NOT NULL,
  PRIMARY KEY (job_id, folder)
);

CREATE TABLE mail (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  job_id INTEGER REFERENCES jobs(id) ON DELETE CASCADE,
  -- whose it is: job:N (the lead), assignment:N (a worker), tab:ID
  box TEXT NOT NULL,
  sender TEXT NOT NULL,
  -- report, question, answer, note, alert, decision, reply
  kind TEXT NOT NULL,
  subject TEXT NOT NULL,
  body TEXT NOT NULL,
  -- what goes with it, as JSON
  extra TEXT NOT NULL DEFAULT '{}',
  is_read INTEGER NOT NULL DEFAULT 0,
  -- the box's tab has been told it is there
  is_told INTEGER NOT NULL DEFAULT 0,
  sent_at INTEGER NOT NULL
);

CREATE TABLE opened_tabs (
  tab TEXT NOT NULL,
  job_id INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
  -- job (the job's to close), person (a person typed into it), gone
  held_by TEXT NOT NULL,
  opened_at INTEGER NOT NULL,
  PRIMARY KEY (tab, job_id)
);

CREATE TABLE questions (
  -- the mail that asked it
  id INTEGER PRIMARY KEY,
  job_id INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
  assignment_id INTEGER NOT NULL REFERENCES assignments(id) ON DELETE CASCADE,
  question TEXT NOT NULL,
  choices TEXT NOT NULL DEFAULT '[]',
  -- asked, answered, closed (its assignment ended first)
  state TEXT NOT NULL,
  answer TEXT,
  asked_at INTEGER NOT NULL,
  answered_at INTEGER
);

CREATE TABLE raised (
  key TEXT PRIMARY KEY,
  raised_at INTEGER NOT NULL
);

CREATE TABLE tasks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  job_id INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  -- the tasks that must be done first, as a JSON list of ids
  waits_on TEXT NOT NULL DEFAULT '[]',
  -- waiting (on others), open, working, done, failed, held (for the lead)
  state TEXT NOT NULL,
  -- what the worker reported, as JSON
  result TEXT,
  -- times a tab's program was lost while doing it
  losses INTEGER NOT NULL DEFAULT 0,
  -- why it is held or failed, in a sentence
  why TEXT,
  created_at INTEGER NOT NULL,
  finished_at INTEGER
);

CREATE INDEX assignments_by_tab ON assignments(tab, state);

CREATE INDEX mail_unread ON mail(box, is_read, seq);

CREATE UNIQUE INDEX one_handover_given ON handovers(box) WHERE state = 'given';
