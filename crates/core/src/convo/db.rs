//! The record of conversations, kept on disk (`conversations.db`).
//!
//! What is written down is what a CLI's own record of a conversation cannot
//! say, because only this app saw it happen:
//!
//! * **conversations** -- which of the CLI's conversations a tab carried on,
//!   in order, so everything said in one tab reads as one thread across a
//!   `/clear` or a conversation picked back up; and whether the tab ran
//!   without asking first.
//! * **sends** -- every piece of text this app put into a tab, and whose it
//!   was. In the CLI's record all of them read as the person's words.
//! * **spans** -- how long a tab spent in each state, a row a change.
//! * **answers** -- who answered a tab that was waiting on a question.
//! * **stops** -- who stopped a tab, from where, and why.
//! * **asks**, **lines**, **reactions**, **shares** -- AIs conferring: each
//!   ask one tab made of another, the short lines said about it, the marks
//!   put on them and the cards shared, shown together by `crate::convo::confer`.
//!
//! The words of a conversation are not kept here: they are in the CLI's
//! record, and a second copy would be a second thing to drift and to leak.
//! The conference is the exception, and its tables say why. A send
//! keeps fingerprints of how its text begins ([`heads`]), which is what finds
//! it in the record again.
//!
//! Made and changed by the numbered steps in `migrations/`, opened the way
//! every SQLite file of this app is (`crate::sqlite`). What the steps add up
//! to is `docs/design/conversations-db.sql`, kept in step by a test.
//!
//! **One writer.** Everything that writes runs on the app's main loop. The
//! panel's reads run on a thread of their own, over a read-only connection
//! ([`Store::open_read`]), which WAL lets run beside the writer.

use std::path::Path;

use anyhow::Result;
use rusqlite::{Connection, OpenFlags, OptionalExtension as _, params};
use sha2::{Digest as _, Sha256};

pub use crate::sqlite::{Step, now_ms};

/// What the record is called where a refusal names it
const WHAT: &str = "the record of conversations";

/// Every change to the tables, in order. A step is never edited once it has
/// shipped: a record that already has it would not run it again. The number is
/// the version a record is at once the step has run
pub const STEPS: &[(i64, &str, Step)] = &[
    (1, "first", Step::Sql(include_str!("migrations/0001_first.sql"))),
    (2, "confer", Step::Sql(include_str!("migrations/0002_confer.sql"))),
];

/// The version the steps bring a record to
pub fn latest() -> i64 {
    STEPS.last().map(|s| s.0).unwrap_or(0)
}

/// How long anything is kept. Long enough to look back over a season of
/// work; every CLI forgets its own records well before this (Claude Code
/// after 30 days unless told otherwise), and what is kept here only means
/// something beside those
pub const KEEP_MS: i64 = 90 * 24 * 60 * 60 * 1000;

/// How much of a text a fingerprint is taken from, after its runs of space are
/// made single: enough to tell two things apart, short enough that a CLI
/// trimming a long paste still leaves it whole
pub const HEAD_CHARS: usize = 60;

/// The fingerprint of how `text` begins: its first [`HEAD_CHARS`] characters,
/// runs of space made single, hashed. What a send is found by in the CLI's
/// record, without the words being kept
pub fn head(text: &str) -> Option<String> {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(HEAD_CHARS).collect();
    if flat.is_empty() {
        return None;
    }
    let digest = Sha256::digest(flat.as_bytes());
    Some(digest.iter().take(8).map(|b| format!("{b:02x}")).collect())
}

/// The fingerprints of every way a text may land in the record (see
/// [`head`]), each once
pub fn heads(texts: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for h in texts.iter().filter_map(|t| head(t)) {
        if !out.contains(&h) {
            out.push(h);
        }
    }
    out
}

/// Who put words into a tab, answered it, or stopped it. Written as the
/// words in the `by` columns
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum By {
    /// A person, at the window or from afar
    Person,
    /// Another tab: an AI that asked it, or a script that tab ran
    Tab,
    /// A job being seen through (orchestration)
    Job,
    /// The lead of a job, stopping its own workers
    Lead,
    /// The tab's CLI itself, at a usage limit
    Limit,
    /// Automation that belongs to no tab: the person's Lua, a caller from
    /// outside every tab
    Automation,
}

impl By {
    pub fn as_str(self) -> &'static str {
        match self {
            By::Person => "person",
            By::Tab => "tab",
            By::Job => "job",
            By::Lead => "lead",
            By::Limit => "limit",
            By::Automation => "automation",
        }
    }
}

/// Where a person was when they did it
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    /// At the app's own window
    Window,
    /// From afar: the phone, or any page the app serves over the network
    Phone,
}

impl Device {
    pub fn as_str(self) -> &'static str {
        match self {
            Device::Window => "window",
            Device::Phone => "phone",
        }
    }
}

/// Where something put into a tab came from
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub by: By,
    pub device: Option<Device>,
    /// The way it came: composer / reply / quick / keys / ask / script /
    /// brief / mail
    pub via: &'static str,
    /// The tab that sent it (by a tab), or the job's lead (by a job)
    pub sender: Option<String>,
    /// The job's number (by a job)
    pub job: Option<i64>,
}

impl Origin {
    /// A person, by `via`, from `device`
    pub fn person(device: Device, via: &'static str) -> Self {
        Origin { by: By::Person, device: Some(device), via, sender: None, job: None }
    }

    /// Another tab (`sender`), by `via`
    pub fn tab(sender: &str, via: &'static str) -> Self {
        Origin { by: By::Tab, device: None, via, sender: Some(sender.to_string()), job: None }
    }

    /// Automation that belongs to no tab
    pub fn automation(via: &'static str) -> Self {
        Origin { by: By::Automation, device: None, via, sender: None, job: None }
    }

    /// A job, led by `lead`
    pub fn job(job: Option<i64>, lead: Option<&str>, via: &'static str) -> Self {
        Origin { by: By::Job, device: None, via, sender: lead.map(str::to_string), job }
    }
}

/// One time a tab was stopped
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stop {
    pub by: By,
    pub device: Option<Device>,
    /// all (the stop button, every tab at once) / esc / job / limit
    pub how: &'static str,
    pub job: Option<i64>,
    pub why: Option<String>,
}

// -- rows, as the panel reads them ---------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Conversation {
    pub id: i64,
    pub tab: String,
    pub cli: String,
    pub record_id: String,
    pub is_yolo: bool,
    pub first_at: i64,
    pub last_at: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Send {
    pub id: i64,
    pub tab: String,
    pub record_id: Option<String>,
    pub sent_at: i64,
    pub by: String,
    pub device: Option<String>,
    pub via: String,
    pub sender: Option<String>,
    pub job: Option<i64>,
    pub heads: Vec<String>,
    pub chars: i64,
}

/// A wait for an answer, and who gave it
#[derive(Clone, Debug, PartialEq)]
pub struct Wait {
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub answered_at: Option<i64>,
    pub by: Option<String>,
    pub device: Option<String>,
    pub via: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stopped {
    pub stopped_at: i64,
    pub by: String,
    pub device: Option<String>,
    pub how: String,
    pub job: Option<i64>,
    pub why: Option<String>,
}

/// An ask, as a line of the conference carries it
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct AskRow {
    pub id: i64,
    pub caller: Option<String>,
    pub target: String,
    pub text: String,
    pub reply: Option<String>,
    pub state: String,
    pub round: i64,
}

/// A mark on a line, and who put it there
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Mark {
    pub by: String,
    pub mark: String,
}

/// One thing in the conference, as the panel shows it
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "k", rename_all = "lowercase")]
pub enum Said {
    Line {
        id: i64,
        tab: Option<String>,
        at: i64,
        text: String,
        how: String,
        ask: Option<AskRow>,
        marks: Vec<Mark>,
    },
    Share {
        id: i64,
        tab: String,
        at: i64,
        kind: String,
        target: String,
        title: String,
        detail: serde_json::Value,
    },
}

impl Said {
    pub fn at(&self) -> i64 {
        match self {
            Said::Line { at, .. } | Said::Share { at, .. } => *at,
        }
    }

    /// Of two things at the same moment, which came second: a card after the
    /// line said with it, then the order they were written in
    fn order(&self) -> (u8, i64) {
        match self {
            Said::Line { id, .. } => (0, *id),
            Said::Share { id, .. } => (1, *id),
        }
    }
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// The record at `path`, made if it is not there and brought up to date if
    /// an older version wrote it. What is past its keeping is let go
    pub fn open(path: &Path) -> Result<Self> {
        Self::with(crate::sqlite::open(path, STEPS, WHAT)?)
    }

    /// A record that lives only as long as this value (tests)
    pub fn in_memory() -> Result<Self> {
        Self::with(crate::sqlite::in_memory(STEPS, WHAT)?)
    }

    fn with(conn: Connection) -> Result<Self> {
        let s = Self { conn };
        s.forget_old(now_ms())?;
        Ok(s)
    }

    /// A connection that only reads, for a thread beside the main loop. It
    /// does not bring the file up to date -- the writer did that when it
    /// opened -- and it refuses a file it does not know the version of
    pub fn open_read(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let have: i64 = conn
            .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |r| r.get::<_, String>(0))
            .optional()?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if have != latest() {
            anyhow::bail!("{WHAT} is at version {have}; this build reads version {}", latest());
        }
        Ok(Self { conn })
    }

    /// What is older than [`KEEP_MS`] before `now`
    pub fn forget_old(&self, now: i64) -> Result<()> {
        let before = now - KEEP_MS;
        self.conn.execute_batch("BEGIN")?;
        let done = (|| -> Result<()> {
            self.conn.execute("DELETE FROM conversations WHERE last_at < ?1", params![before])?;
            self.conn.execute("DELETE FROM sends WHERE sent_at < ?1", params![before])?;
            self.conn
                .execute("DELETE FROM spans WHERE ended_at IS NOT NULL AND ended_at < ?1", params![before])?;
            self.conn.execute("DELETE FROM stops WHERE stopped_at < ?1", params![before])?;
            // A line's marks go with it (ON DELETE CASCADE)
            self.conn.execute("DELETE FROM lines WHERE said_at < ?1", params![before])?;
            // An ask answered long after it was made stays as long as its
            // answer's line does: the line opens onto its whole text
            self.conn.execute(
                "DELETE FROM asks WHERE asked_at < ?1 AND NOT EXISTS (SELECT 1 FROM lines WHERE lines.ask_id = asks.id)",
                params![before],
            )?;
            self.conn.execute("DELETE FROM shares WHERE shared_at < ?1", params![before])?;
            Ok(())
        })();
        match done {
            Ok(()) => self.conn.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        Ok(())
    }

    // -- writing (the main loop) -----------------------------------------------

    /// A tab seen on a conversation at `at`: made the first time, its last
    /// sighting moved on after that
    pub fn seen(&self, tab: &str, cli: &str, record_id: &str, is_yolo: bool, at: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO conversations (tab, cli, record_id, is_yolo, first_at, last_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5) \
             ON CONFLICT(tab, record_id) DO UPDATE SET last_at = MAX(last_at, excluded.last_at), \
             is_yolo = MAX(is_yolo, excluded.is_yolo)",
            params![tab, cli, record_id, is_yolo as i64, at],
        )?;
        Ok(())
    }

    /// A conversation whose record is gone from the machine it was on, with
    /// what was sent in it: nothing it says can be shown beside a record that
    /// is not there
    pub fn forget_conversation(&self, tab: &str, record_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM sends WHERE tab = ?1 AND record_id = ?2", params![tab, record_id])?;
        self.conn
            .execute("DELETE FROM conversations WHERE tab = ?1 AND record_id = ?2", params![tab, record_id])?;
        Ok(())
    }

    pub fn sent(&self, tab: &str, record_id: Option<&str>, texts: &[&str], from: &Origin, at: i64) -> Result<()> {
        let heads = serde_json::to_string(&heads(texts))?;
        let chars = texts.first().map_or(0, |t| t.chars().count()) as i64;
        self.conn.execute(
            "INSERT INTO sends (tab, record_id, sent_at, by, device, via, sender, job, heads, chars) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                tab,
                record_id,
                at,
                from.by.as_str(),
                from.device.map(Device::as_str),
                from.via,
                from.sender,
                from.job,
                heads,
                chars
            ],
        )?;
        Ok(())
    }

    /// A tab changed state at `at`: the stretch it was in ends, a new one
    /// begins. One transaction, so there is never a moment with two open
    pub fn state(&mut self, tab: &str, state: &str, at: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("UPDATE spans SET ended_at = ?2 WHERE tab = ?1 AND ended_at IS NULL", params![tab, at])?;
        tx.execute("INSERT INTO spans (tab, state, started_at) VALUES (?1, ?2, ?3)", params![tab, state, at])?;
        tx.commit()?;
        Ok(())
    }

    /// Input reached a tab at `at`. When the tab is waiting on a question
    /// nobody has answered yet, this is the answer; anything after it is not
    pub fn touched(&self, tab: &str, from: &Origin, at: i64) -> Result<bool> {
        let open: Option<i64> = self
            .conn
            .query_row(
                "SELECT s.id FROM spans s LEFT JOIN answers a ON a.span_id = s.id \
                 WHERE s.tab = ?1 AND s.ended_at IS NULL AND s.state = 'QUESTION' AND a.id IS NULL",
                params![tab],
                |r| r.get(0),
            )
            .optional()?;
        let Some(span) = open else { return Ok(false) };
        self.conn.execute(
            "INSERT INTO answers (span_id, answered_at, by, device, via) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![span, at, from.by.as_str(), from.device.map(Device::as_str), from.via],
        )?;
        Ok(true)
    }

    pub fn stopped(&self, tab: &str, stop: &Stop, at: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO stops (tab, stopped_at, by, device, how, job, why) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![tab, at, stop.by.as_str(), stop.device.map(Device::as_str), stop.how, stop.job, stop.why],
        )?;
        Ok(())
    }

    /// The state a tab is in now, as last written
    pub fn state_now(&self, tab: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT state FROM spans WHERE tab = ?1 AND ended_at IS NULL", params![tab], |r| r.get(0))
            .optional()?)
    }

    /// Every stretch still open ends at `at`: the app is closing, and a tab
    /// that is not watched is in no state anybody knows
    pub fn close_spans(&self, at: i64) -> Result<()> {
        self.conn.execute("UPDATE spans SET ended_at = ?1 WHERE ended_at IS NULL", params![at])?;
        Ok(())
    }

    // -- the conference (the main loop) ------------------------------------------

    /// An ask sent: `caller` asked `target` on `desk`. Its id, for its lines
    pub fn ask_opened(&self, desk: &str, caller: Option<&str>, target: &str, text: &str, round: u32, at: i64) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO asks (desk, caller, target, text, state, round, asked_at) VALUES (?1, ?2, ?3, ?4, 'waiting', ?5, ?6)",
            params![desk, caller, target, text, round, at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// How an ask ended, and what was said back
    pub fn ask_answered(&self, id: i64, state: &str, reply: Option<&str>, at: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE asks SET state = ?2, reply = COALESCE(?3, reply), answered_at = ?4 WHERE id = ?1",
            params![id, state, reply, at],
        )?;
        Ok(())
    }

    /// A line said. Its id, for the marks put on it
    pub fn line(&self, desk: &str, tab: Option<&str>, text: &str, ask: Option<i64>, how: &str, at: i64) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO lines (desk, tab, said_at, text, ask_id, how) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![desk, tab, at, text, ask, how],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Whether the answer to `ask` has a line yet, in its own words or taken for it
    pub fn ask_has_answer_line(&self, ask: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM lines WHERE ask_id = ?1 AND how IN ('said', 'auto') LIMIT 1",
                params![ask],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// The last line `tab` said on `desk` (`None`: the person's). A decision
    /// is nobody's line to answer
    pub fn last_line_of(&self, desk: &str, tab: Option<&str>) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM lines WHERE desk = ?1 AND tab IS ?2 AND how != 'agreed' ORDER BY said_at DESC, id DESC LIMIT 1",
                params![desk, tab],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The desk a line was said on, if it is still kept
    pub fn desk_of_line(&self, line: i64) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT desk FROM lines WHERE id = ?1", params![line], |r| r.get(0)).optional()?)
    }

    /// A mark put on a line, or taken off again when `by` had already put
    /// the same one there. `true`: it is on now
    pub fn toggle_mark(&self, line: i64, by: &str, mark: &str, at: i64) -> Result<bool> {
        let gone = self
            .conn
            .execute("DELETE FROM reactions WHERE line_id = ?1 AND by = ?2 AND mark = ?3", params![line, by, mark])?;
        if gone > 0 {
            return Ok(false);
        }
        self.conn.execute(
            "INSERT INTO reactions (line_id, by, mark, marked_at) VALUES (?1, ?2, ?3, ?4)",
            params![line, by, mark, at],
        )?;
        Ok(true)
    }

    /// A card shared
    #[allow(clippy::too_many_arguments)]
    pub fn shared(
        &self,
        desk: &str,
        tab: &str,
        kind: &str,
        target: &str,
        title: &str,
        detail: &serde_json::Value,
        at: i64,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO shares (desk, tab, shared_at, kind, target, title, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![desk, tab, at, kind, target, title, detail.to_string()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    // -- the conference (the panel's thread) -----------------------------------

    /// A page of the conference on `desk`: what was said and shared before
    /// `before`, the newest `want` of it, handed back oldest first -- the
    /// way a chat reads
    pub fn conference(&self, desk: &str, before: i64, want: usize) -> Result<Vec<Said>> {
        let mut out: Vec<Said> = Vec::new();
        let mut st = self.conn.prepare(
            "SELECT l.id, l.tab, l.said_at, l.text, l.how, a.id, a.caller, a.target, a.text, a.reply, a.state, a.round \
             FROM lines l LEFT JOIN asks a ON a.id = l.ask_id \
             WHERE l.desk = ?1 AND l.said_at < ?2 ORDER BY l.said_at DESC, l.id DESC LIMIT ?3",
        )?;
        let lines = st
            .query_map(params![desk, before, want as i64], |r| {
                let ask = match r.get::<_, Option<i64>>(5)? {
                    None => None,
                    Some(id) => Some(AskRow {
                        id,
                        caller: r.get(6)?,
                        target: r.get(7)?,
                        text: r.get(8)?,
                        reply: r.get(9)?,
                        state: r.get(10)?,
                        round: r.get(11)?,
                    }),
                };
                Ok(Said::Line {
                    id: r.get(0)?,
                    tab: r.get(1)?,
                    at: r.get(2)?,
                    text: r.get(3)?,
                    how: r.get(4)?,
                    ask,
                    marks: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        out.extend(lines);
        let mut st = self.conn.prepare(
            "SELECT id, tab, shared_at, kind, target, title, detail FROM shares \
             WHERE desk = ?1 AND shared_at < ?2 ORDER BY shared_at DESC, id DESC LIMIT ?3",
        )?;
        let shares = st
            .query_map(params![desk, before, want as i64], |r| {
                Ok(Said::Share {
                    id: r.get(0)?,
                    tab: r.get(1)?,
                    at: r.get(2)?,
                    kind: r.get(3)?,
                    target: r.get(4)?,
                    title: r.get(5)?,
                    detail: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        out.extend(shares);
        // Newest first to cut the page, then the other way round to read it
        out.sort_by(|a, b| b.at().cmp(&a.at()).then(b.order().cmp(&a.order())));
        out.truncate(want);
        out.reverse();
        let mut st = self.conn.prepare("SELECT by, mark FROM reactions WHERE line_id = ?1 ORDER BY marked_at, id")?;
        for s in out.iter_mut() {
            if let Said::Line { id, marks, .. } = s {
                *marks = st
                    .query_map(params![*id], |r| Ok(Mark { by: r.get(0)?, mark: r.get(1)? }))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
            }
        }
        Ok(out)
    }

    // -- reading (the panel's thread) ------------------------------------------

    const CONVERSATION_COLS: &'static str = "id, tab, cli, record_id, is_yolo, first_at, last_at";

    fn conversation_row(r: &rusqlite::Row) -> rusqlite::Result<Conversation> {
        Ok(Conversation {
            id: r.get(0)?,
            tab: r.get(1)?,
            cli: r.get(2)?,
            record_id: r.get(3)?,
            is_yolo: r.get::<_, i64>(4)? != 0,
            first_at: r.get(5)?,
            last_at: r.get(6)?,
        })
    }

    /// The tab a conversation was carried on, by the CLI's id for it: the
    /// last tab seen on it
    pub fn tab_of(&self, record_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT tab FROM conversations WHERE record_id = ?1 ORDER BY last_at DESC LIMIT 1",
                params![record_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Every conversation of the tab that carried `record_id`, the newest
    /// first: the one thread a `/clear` or a conversation picked back up
    /// split into several records
    pub fn chain_of(&self, record_id: &str) -> Result<Vec<Conversation>> {
        match self.tab_of(record_id)? {
            Some(tab) => self.conversations(&tab),
            None => Ok(Vec::new()),
        }
    }

    /// The conversations a tab carried on, the newest first
    pub fn conversations(&self, tab: &str) -> Result<Vec<Conversation>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM conversations WHERE tab = ?1 ORDER BY first_at DESC, id DESC",
            Self::CONVERSATION_COLS
        ))?;
        let rows = st.query_map(params![tab], Self::conversation_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    const SEND_COLS: &'static str = "id, tab, record_id, sent_at, by, device, via, sender, job, heads, chars";

    fn send_row(r: &rusqlite::Row) -> rusqlite::Result<Send> {
        Ok(Send {
            id: r.get(0)?,
            tab: r.get(1)?,
            record_id: r.get(2)?,
            sent_at: r.get(3)?,
            by: r.get(4)?,
            device: r.get(5)?,
            via: r.get(6)?,
            sender: r.get(7)?,
            job: r.get(8)?,
            heads: serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or_default(),
            chars: r.get(10)?,
        })
    }

    /// What was sent into a tab between `from` and `to` (inclusive), oldest first
    pub fn sends(&self, tab: &str, from: i64, to: i64) -> Result<Vec<Send>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM sends WHERE tab = ?1 AND sent_at BETWEEN ?2 AND ?3 ORDER BY sent_at, id",
            Self::SEND_COLS
        ))?;
        let rows = st.query_map(params![tab, from, to], Self::send_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The waits for an answer a tab had that began in `(after, until]`,
    /// oldest first
    pub fn waits(&self, tab: &str, after: i64, until: i64) -> Result<Vec<Wait>> {
        let mut st = self.conn.prepare(
            "SELECT s.started_at, s.ended_at, a.answered_at, a.by, a.device, a.via FROM spans s \
             LEFT JOIN answers a ON a.span_id = s.id \
             WHERE s.tab = ?1 AND s.state = 'QUESTION' AND s.started_at > ?2 AND s.started_at <= ?3 \
             ORDER BY s.started_at, s.id",
        )?;
        let rows = st
            .query_map(params![tab, after, until], |r| {
                Ok(Wait {
                    started_at: r.get(0)?,
                    ended_at: r.get(1)?,
                    answered_at: r.get(2)?,
                    by: r.get(3)?,
                    device: r.get(4)?,
                    via: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The times a tab was stopped in `(after, until]`, oldest first
    pub fn stops(&self, tab: &str, after: i64, until: i64) -> Result<Vec<Stopped>> {
        let mut st = self.conn.prepare(
            "SELECT stopped_at, by, device, how, job, why FROM stops \
             WHERE tab = ?1 AND stopped_at > ?2 AND stopped_at <= ?3 ORDER BY stopped_at, id",
        )?;
        let rows = st
            .query_map(params![tab, after, until], |r| {
                Ok(Stopped {
                    stopped_at: r.get(0)?,
                    by: r.get(1)?,
                    device: r.get(2)?,
                    how: r.get(3)?,
                    job: r.get(4)?,
                    why: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::{migrate, schema, version_of};
    use rusqlite::Transaction;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("convo-{name}-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn me() -> Origin {
        Origin::person(Device::Phone, "composer")
    }

    #[test]
    fn a_fingerprint_is_of_how_a_text_begins_whatever_its_spacing() {
        assert_eq!(head("fix  the\ntest"), head("fix the test"));
        assert_ne!(head("fix the test"), head("fix the docs"));
        assert_eq!(head("   "), None);
        let long = "x".repeat(500);
        assert_eq!(head(&long), head(&format!("{long} and more")), "only the start counts");
        assert_eq!(heads(&["a b", "a  b", "c"]).len(), 2, "each once");
    }

    #[test]
    fn a_question_is_answered_once_by_the_first_input_that_reaches_it() {
        let mut s = Store::in_memory().unwrap();
        assert!(!s.touched("t", &me(), 5).unwrap(), "nothing is waiting");
        s.state("t", "BUSY", 10).unwrap();
        s.state("t", "QUESTION", 20).unwrap();
        assert!(s.touched("t", &me(), 30).unwrap());
        assert!(!s.touched("t", &Origin::automation("script"), 31).unwrap(), "only the first answers");
        s.state("t", "BUSY", 40).unwrap();
        let w = s.waits("t", 0, 100).unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!(
            (w[0].started_at, w[0].ended_at, w[0].answered_at, w[0].by.as_deref(), w[0].device.as_deref()),
            (20, Some(40), Some(30), Some("person"), Some("phone"))
        );
        assert_eq!(s.state_now("t").unwrap().as_deref(), Some("BUSY"));
    }

    #[test]
    fn a_tab_is_in_one_state_at_a_time() {
        let mut s = Store::in_memory().unwrap();
        for (i, st) in ["BUSY", "DONE", "BUSY", "QUESTION"].iter().enumerate() {
            s.state("t", st, i as i64).unwrap();
        }
        s.state("u", "BUSY", 9).unwrap();
        let open: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM spans WHERE tab = 't' AND ended_at IS NULL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(open, 1);
        s.close_spans(50).unwrap();
        assert_eq!(s.state_now("t").unwrap(), None);
    }

    #[test]
    fn what_is_past_its_keeping_is_let_go() {
        let mut s = Store::in_memory().unwrap();
        let now = 10 * KEEP_MS;
        s.seen("t", "claude", "old", false, now - KEEP_MS - 1).unwrap();
        s.seen("t", "claude", "new", false, now).unwrap();
        s.sent("t", Some("old"), &["hi"], &me(), now - KEEP_MS - 1).unwrap();
        s.sent("t", Some("new"), &["hi"], &me(), now).unwrap();
        s.state("t", "QUESTION", now - KEEP_MS - 5).unwrap();
        s.touched("t", &me(), now - KEEP_MS - 4).unwrap();
        s.state("t", "DONE", now - KEEP_MS - 3).unwrap();
        s.stopped("t", &Stop { by: By::Person, device: None, how: "esc", job: None, why: None }, now - KEEP_MS - 2)
            .unwrap();
        s.forget_old(now).unwrap();
        let kept: Vec<String> = s.conversations("t").unwrap().into_iter().map(|c| c.record_id).collect();
        assert_eq!(kept, vec!["new"]);
        assert_eq!(s.sends("t", 0, i64::MAX).unwrap().len(), 1);
        assert!(s.waits("t", i64::MIN, i64::MAX).unwrap().is_empty(), "the answer went with its wait");
        assert!(s.stops("t", i64::MIN, i64::MAX).unwrap().is_empty());
        assert_eq!(s.state_now("t").unwrap().as_deref(), Some("DONE"), "the stretch still open is kept");
    }

    #[test]
    fn a_conversation_is_made_once_and_its_last_sighting_moves_on() {
        let s = Store::in_memory().unwrap();
        s.seen("t", "claude", "a", false, 10).unwrap();
        s.seen("t", "claude", "a", true, 30).unwrap();
        s.seen("t", "claude", "a", false, 20).unwrap();
        s.seen("t", "claude", "b", false, 40).unwrap();
        let c = s.conversations("t").unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].record_id.as_str(), c[1].record_id.as_str()), ("b", "a"), "the newest first");
        assert_eq!((c[1].first_at, c[1].last_at, c[1].is_yolo), (10, 30, true));
        s.sent("t", Some("a"), &["x"], &me(), 11).unwrap();
        s.forget_conversation("t", "a").unwrap();
        assert_eq!(s.conversations("t").unwrap().len(), 1);
        assert!(s.sends("t", 0, 100).unwrap().is_empty());
    }

    #[test]
    fn the_reader_reads_beside_the_writer() {
        let dir = scratch("read");
        let path = dir.join("conversations.db");
        let s = Store::open(&path).unwrap();
        s.sent("t", None, &["hello"], &Origin::tab("lead", "ask"), 5).unwrap();
        let r = Store::open_read(&path).unwrap();
        let got = r.sends("t", 0, 10).unwrap();
        assert_eq!((got[0].by.as_str(), got[0].sender.as_deref(), got[0].heads.clone()), ("tab", Some("lead"), heads(&["hello"])));
        assert!(r.seen("t", "c", "r", false, 1).is_err(), "the reader cannot write");
        drop((s, r));
        let _ = std::fs::remove_dir_all(dir);
    }

    // -- changing the tables (docs/design/sqlite.md section 8) -------------------

    fn with_one_more(name: &'static str, step: Step) -> Vec<(i64, &'static str, Step)> {
        let mut steps: Vec<(i64, &'static str, Step)> = STEPS
            .iter()
            .map(|(n, name, s)| {
                let s = match s {
                    Step::Sql(sql) => Step::Sql(sql),
                    Step::Code(f) => Step::Code(*f),
                };
                (*n, *name, s)
            })
            .collect();
        steps.push((latest() + 1, name, step));
        steps
    }

    /// A later version that adds a column and rebuilds a table SQLite cannot
    /// change in place -- the kinds of change a real step makes
    fn next_version() -> Vec<(i64, &'static str, Step)> {
        fn reshape(tx: &Transaction) -> Result<()> {
            tx.execute_batch("ALTER TABLE sends ADD COLUMN lines INTEGER NOT NULL DEFAULT 0;")?;
            crate::sqlite::rebuild(
                tx,
                "stops",
                "CREATE TABLE {table} (id INTEGER PRIMARY KEY, tab TEXT NOT NULL, stopped_at INTEGER NOT NULL, \
                 by TEXT NOT NULL, device TEXT, how TEXT NOT NULL, job INTEGER, why TEXT, seen INTEGER NOT NULL DEFAULT 0)",
                "id, tab, stopped_at, by, device, how, job, why",
                &["CREATE INDEX stops_by_tab ON stops (tab, stopped_at)"],
            )
        }
        with_one_more("reshape", Step::Code(reshape))
    }

    fn migrated(steps: &[(i64, &str, Step)]) -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn, steps, None, WHAT).unwrap();
        conn
    }

    #[test]
    fn a_new_record_is_made_by_the_steps_alone() {
        let mut conn = migrated(STEPS);
        assert_eq!(version_of(&conn).unwrap(), latest());
        let before = schema(&conn).unwrap();
        migrate(&mut conn, STEPS, None, WHAT).unwrap();
        assert_eq!(schema(&conn).unwrap(), before, "running them again changes nothing");
    }

    #[test]
    fn an_older_record_is_copied_then_brought_up_to_date_with_its_rows() {
        let dir = scratch("upgrade");
        let path = dir.join("conversations.db");
        {
            let mut s = Store::open(&path).unwrap();
            let now = now_ms();
            s.seen("t", "claude", "a", true, now).unwrap();
            s.sent("t", Some("a"), &["keep me"], &me(), now).unwrap();
            s.state("t", "QUESTION", now).unwrap();
            s.touched("t", &me(), now + 1).unwrap();
            s.stopped("t", &Stop { by: By::Limit, device: None, how: "limit", job: None, why: Some("resets 3pm".into()) }, now)
                .unwrap();
        }
        let now = latest();
        let backup = dir.join(format!("conversations.db.v{now}.bak"));
        let mut conn = Connection::open(&path).unwrap();
        migrate(&mut conn, &next_version(), Some(&backup), WHAT).unwrap();
        assert_eq!(version_of(&conn).unwrap(), now + 1);
        let (heads_kept, lines): (String, i64) = conn
            .query_row("SELECT heads, lines FROM sends WHERE tab = 't'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((heads_kept, lines), (serde_json::to_string(&heads(&["keep me"])).unwrap(), 0));
        let (why, seen): (String, i64) = conn
            .query_row("SELECT why, seen FROM stops WHERE tab = 't'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((why.as_str(), seen), ("resets 3pm", 0), "the rebuilt table kept its rows");
        let answered: i64 = conn.query_row("SELECT COUNT(*) FROM answers", [], |r| r.get(0)).unwrap();
        assert_eq!(answered, 1, "what pointed at a span still does");
        drop(conn);
        let old = Connection::open(&backup).unwrap();
        assert_eq!(version_of(&old).unwrap(), now, "the copy is the record as it was");
        drop(old);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_step_that_fails_leaves_the_record_as_it_was() {
        fn broken(tx: &Transaction) -> Result<()> {
            tx.execute_batch("ALTER TABLE sends ADD COLUMN half INTEGER;")?;
            anyhow::bail!("the second half went wrong")
        }
        let mut conn = migrated(STEPS);
        let before = schema(&conn).unwrap();
        let e = migrate(&mut conn, &with_one_more("broken", Step::Code(broken)), None, WHAT).unwrap_err();
        assert!(format!("{e:#}").contains("(broken)"), "{e:#}");
        assert_eq!(version_of(&conn).unwrap(), latest());
        assert_eq!(schema(&conn).unwrap(), before, "nothing of the step is left");
    }

    #[test]
    fn a_record_from_a_newer_version_is_left_alone() {
        let mut conn = migrated(&next_version());
        let e = migrate(&mut conn, STEPS, None, WHAT).unwrap_err();
        assert!(e.to_string().contains("newer version"), "{e}");
        assert_eq!(version_of(&conn).unwrap(), latest() + 1);
    }

    #[test]
    fn a_step_that_leaves_rows_pointing_at_nothing_is_refused() {
        fn orphan(tx: &Transaction) -> Result<()> {
            tx.execute_batch("INSERT INTO answers (span_id, answered_at, by, via) VALUES (999, 0, 'person', 'keys');")?;
            Ok(())
        }
        let mut conn = migrated(STEPS);
        let e = migrate(&mut conn, &with_one_more("orphan", Step::Code(orphan)), None, WHAT).unwrap_err();
        assert!(e.to_string().contains("pointing at nothing"), "{e}");
        assert_eq!(version_of(&conn).unwrap(), latest());
    }

    #[test]
    fn the_conference_reads_as_a_chat_oldest_first_with_its_asks_and_marks() {
        let s = Store::in_memory().unwrap();
        let ask = s.ask_opened("d", Some("otter"), "finch", "Review the parser in src/p.rs", 1, 10).unwrap();
        let asked = s.line("d", Some("otter"), "Can you review the parser?", Some(ask), "ask", 10).unwrap();
        assert!(!s.ask_has_answer_line(ask).unwrap());
        s.ask_answered(ask, "DONE", Some("Two findings: ..."), 30).unwrap();
        let said = s.line("d", Some("finch"), "Two small things, fixable.", Some(ask), "said", 30).unwrap();
        assert!(s.ask_has_answer_line(ask).unwrap());
        s.shared("d", "finch", "commit", "abc1234", "Fix the parser", &serde_json::json!({"branch": "main"}), 30).unwrap();
        s.line("elsewhere", Some("otter"), "not this desk", None, "aside", 20).unwrap();
        assert!(s.toggle_mark(said, "otter", "👍", 40).unwrap());
        assert!(s.toggle_mark(said, "person", "👍", 41).unwrap());
        assert!(!s.toggle_mark(said, "person", "👍", 42).unwrap(), "the same mark again takes it off");

        let page = s.conference("d", i64::MAX, 10).unwrap();
        assert_eq!(page.len(), 3, "{page:?}");
        let Said::Line { id, how, ask: Some(a), .. } = &page[0] else { panic!("{:?}", page[0]) };
        assert_eq!((*id, how.as_str(), a.target.as_str(), a.state.as_str()), (asked, "ask", "finch", "DONE"));
        assert_eq!(a.reply.as_deref(), Some("Two findings: ..."));
        let Said::Line { marks, .. } = &page[1] else { panic!() };
        assert_eq!(marks, &vec![Mark { by: "otter".into(), mark: "👍".into() }]);
        assert!(matches!(&page[2], Said::Share { kind, .. } if kind == "commit"), "a card after the line said with it");

        // Paged from the newest: the page before the last thing is what came earlier
        let older = s.conference("d", 30, 10).unwrap();
        assert_eq!(older.len(), 1);
        assert_eq!(s.last_line_of("d", Some("finch")).unwrap(), Some(said));
        assert_eq!(s.last_line_of("d", None).unwrap(), None, "the person said nothing");
        assert_eq!(s.desk_of_line(said).unwrap().as_deref(), Some("d"));
    }

    #[test]
    fn the_conference_is_let_go_with_the_rest_and_a_line_takes_its_marks() {
        let s = Store::in_memory().unwrap();
        let ask = s.ask_opened("d", None, "finch", "x", 1, 5).unwrap();
        let l = s.line("d", Some("finch"), "ok", Some(ask), "said", 5).unwrap();
        s.toggle_mark(l, "person", "👍", 6).unwrap();
        s.shared("d", "finch", "url", "https://example.com", "Example", &serde_json::json!({}), 5).unwrap();
        s.forget_old(5 + KEEP_MS + 1).unwrap();
        assert!(s.conference("d", i64::MAX, 10).unwrap().is_empty());
        let marks: i64 = s.conn.query_row("SELECT COUNT(*) FROM reactions", [], |r| r.get(0)).unwrap();
        assert_eq!(marks, 0);
    }

    #[test]
    fn an_ask_stays_while_a_line_still_opens_onto_it() {
        let s = Store::in_memory().unwrap();
        let ask = s.ask_opened("d", None, "finch", "the whole question", 1, 0).unwrap();
        s.ask_answered(ask, "DONE", Some("the whole answer"), KEEP_MS).unwrap();
        s.line("d", Some("finch"), "answered late", Some(ask), "said", KEEP_MS).unwrap();
        s.forget_old(KEEP_MS + 10).unwrap();
        let page = s.conference("d", i64::MAX, 10).unwrap();
        let Said::Line { ask: Some(a), .. } = &page[0] else { panic!("{page:?}") };
        assert_eq!(a.reply.as_deref(), Some("the whole answer"));
    }

    #[test]
    fn the_steps_are_numbered_in_order_and_so_are_their_files() {
        crate::sqlite::check_numbering(STEPS, "crates/core/src/convo/migrations");
    }

    /// The tables as the steps make them, written out for anyone reading the
    /// repository. Change a step and this fails until it is written again:
    ///
    ///     SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core conversations_db
    #[test]
    fn the_conversations_db_design_is_what_the_steps_make() {
        let header = format!(
            "-- The record of conversations (conversations.db), as the steps in\n\
             -- crates/core/src/convo/migrations/ leave it at version {}.\n\
             --\n\
             -- Written by a test; do not edit. Change the tables by adding a step (see\n\
             -- conversations-db.md), then write this again:\n\
             --     SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core conversations_db",
            latest()
        );
        crate::sqlite::check_design(STEPS, WHAT, &header, "docs/design/conversations-db.sql", "conversations_db");
    }
}
