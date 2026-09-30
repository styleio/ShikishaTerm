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
);

CREATE INDEX asks_by_desk ON asks (desk, asked_at);

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
);

CREATE INDEX lines_by_desk ON lines (desk, said_at);

CREATE TABLE reactions (
  -- A mark put on a line: by a person pressing it, or by an AI
  id INTEGER PRIMARY KEY,
  line_id INTEGER NOT NULL REFERENCES lines(id) ON DELETE CASCADE,
  by TEXT NOT NULL,                   -- the tab, by id, or 'person'
  mark TEXT NOT NULL,                 -- one of the few the app offers (confer::MARKS)
  marked_at INTEGER NOT NULL,
  UNIQUE (line_id, by, mark)
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
);

CREATE INDEX shares_by_desk ON shares (desk, shared_at);
